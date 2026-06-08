# Decision Graph Benchmark Specification

This benchmark suite validates HiveMind as an organizational decision memory
system. HiveMind does not make organizational decisions. It captures decisions,
supporting evidence, assumptions, rejected options, contradictions, stale
context, and agentic decision traces so future humans and agents can verify new
work against the decision catalogue.

The defensible product claim is:

> HiveMind improves future organizational decision-making workflows by making
> prior decisions, supporting evidence, stale assumptions, and causal update
> paths discoverable and usable at the moment a new large task, user prompt, or
> proposed agentic decision is being considered.

The benchmark may measure downstream decision-making quality, but only in this
bounded sense: given the same task, prompt, or proposed decision, does an agent
or human-facing workflow produce a more grounded decision-support packet when it
can query HiveMind than when it cannot?

## Folder Contract

All benchmark assets live under `benchmarks/decision_graph/`.

- `SPEC.md`: stable benchmark design and scoring contract.
- `README.md`: user-facing run instructions added by the scaffold bead.
- `schemas/`: JSON Schema files for datasets, traces, packets, configs, and
  results.
- `fixtures/`: deterministic smoke datasets and graph snapshots.
- `runner/`: benchmark runner and method adapters.
- `reports/`: generated example reports from smoke fixtures.

Normal project tests may include a small deterministic smoke command, but full
benchmark runs are not part of ordinary unit test cost.

## Benchmark Families

### Phase 1: Discovery Retrieval

Goal: prove that users and agents can discover the right catalogue nodes for
questions about prior decisions.

Example:

```json
{
  "question": "Why did we choose Postgres over Neo4j?",
  "gold_nodes": [
    {"node_id": "decision_17", "role": "decision"},
    {"node_id": "evidence_8", "role": "evidence"},
    {"node_id": "assumption_3", "role": "assumption"},
    {"node_id": "rejected_option_2", "role": "rejected_option"}
  ]
}
```

Methods: BM25, vector search, hybrid search, graph traversal, graph plus hybrid,
graph plus compaction summaries, and graph/plugin related-context summaries.

Primary metrics: Recall@5, Recall@10, MRR, nDCG@10, latency, and token cost.

### Phase 2: Graph Path Reasoning

Goal: prove that the system can recover update paths where one node is
insufficient.

Example gold path:

```text
initial_assumption -> initial_decision -> new_evidence -> revised_decision
```

Primary metrics: path recall, edge accuracy, temporal ordering accuracy, and
contradiction detection.

### Phase 3: Decision-Support Replay

Goal: replay older organizational decisions from a graph snapshot and evaluate
whether HiveMind context improves a support packet for a human decision-maker.

Input includes all allowed graph nodes before a cutoff timestamp. Gold labels
come from a later human-labeled packet, but post-cutoff nodes are hidden from
the evaluated method.

Primary metrics: decision-support packet quality, risk recall, assumption
recall, evidence grounding, context-summary citation quality, and future-leak
prevention.

### Phase 4: Prompt And Agentic-Decision Verification

Goal: evaluate whether large user prompts and proposed agent actions are checked
against the captured catalogue before work proceeds.

The benchmark scores whether the system finds relevant prior decisions,
supporting evidence, stale assumptions, contradicted constraints, missing
evidence risks, and false positives. It also scores whether a proposed agentic
decision should proceed, be revised, be blocked, or be escalated.

### Phase 5: Agent Graph/Plugin Context Summary

Goal: verify the practical integration path: an agent asks the HiveMind
graph/plugin for related context and receives a grounded summary with cited node
IDs that improves downstream verification or support packets.

The benchmark captures the tool-call trace, returned summary, citations,
latency, token cost, unavailable-tool behavior, and downstream packet quality
lift.

## Method Conditions

| Method condition | Inputs available | Expected use | Failure mode to catch |
| --- | --- | --- | --- |
| `no_hivemind_context` | User prompt only | Baseline packet without catalogue help | Hallucinated organizational memory |
| `bm25` | Text index over captured nodes | Lexical discovery baseline | Misses paraphrases and graph structure |
| `vector` | Embedding index over captured nodes | Semantic discovery baseline | Poor citations, hidden model/cost drift |
| `hybrid` | BM25 plus vector search | Strong retrieval baseline | Rank fusion without graph causality |
| `graph_traversal` | Explicit nodes and edges | Causal/path baseline | Finds nearby nodes but not text matches |
| `graph_hybrid` | Retrieval seeds plus graph expansion | Discovery plus graph context | Over-expands into irrelevant context |
| `graph_compaction_summary` | Node summaries plus graph expansion | Summary-based context packing | Summary omits key evidence or staleness |
| `graph_plugin_context_summary` | Agent-facing graph/plugin call | Grounded context summary for decisions and prompts | Missing tool call, unsupported claims, bad citations |

Optional implementations may add more methods, but reports must always preserve
these method names for comparison.

## Dataset Schemas

The scaffold bead should formalize these as JSON Schema files. Every record
includes `dataset_id`, `record_id`, `created_at`, `source`, and optional
`provenance_notes`.

### Graph Snapshot

```json
{
  "snapshot_id": "org_demo_2026_03_01",
  "cutoff_time": "2026-03-01T00:00:00Z",
  "nodes": [
    {
      "node_id": "decision_17",
      "node_type": "decision",
      "title": "Use Postgres as primary store",
      "body": "Decision text and rationale",
      "status": "accepted",
      "created_at": "2026-02-10T12:00:00Z",
      "actor_id": "human:cto",
      "event_origin": "ledger:42"
    }
  ],
  "edges": [
    {
      "edge_id": "edge_91",
      "source": "decision_17",
      "target": "evidence_8",
      "edge_type": "SUPPORTED_BY",
      "created_at": "2026-02-10T12:01:00Z",
      "event_origin": "ledger:43"
    }
  ]
}
```

Allowed `node_type` values for the initial suite: `decision`, `evidence`,
`assumption`, `hypothesis`, `rejected_option`, `constraint`, `risk`,
`actor_action`, `summary`, and `external_reference`.

### Discovery Question

```json
{
  "question_id": "q_postgres_why",
  "question": "Why did we choose Postgres over Neo4j?",
  "snapshot_id": "org_demo_2026_03_01",
  "gold_nodes": [
    {"node_id": "decision_17", "role": "decision", "gain": 3},
    {"node_id": "evidence_8", "role": "evidence", "gain": 2}
  ],
  "expected_node_types": ["decision", "evidence", "assumption", "rejected_option"]
}
```

`gain` is used for nDCG and defaults to `1` when omitted.

### Gold Path

```json
{
  "path_id": "path_arch_reversal",
  "question": "What changed between the original architecture decision and the later reversal?",
  "snapshot_id": "org_demo_2026_04_01",
  "gold_path": ["assumption_1", "decision_7", "evidence_20", "decision_33"],
  "gold_edges": ["ASSUMED_BY", "SUPERSEDED_AFTER", "SUPPORTED_BY"],
  "temporal_order": ["2026-01-10T00:00:00Z", "2026-01-15T00:00:00Z", "2026-03-20T00:00:00Z", "2026-03-22T00:00:00Z"],
  "contradictions": [{"old_node_id": "assumption_1", "new_node_id": "evidence_20"}]
}
```

### User Prompt Verification Case

```json
{
  "case_id": "prompt_move_to_neo4j",
  "prompt": "Migrate the decision graph from Postgres to Neo4j this week.",
  "snapshot_id": "org_demo_2026_04_01",
  "gold_relevant_nodes": ["decision_17", "rejected_option_2", "evidence_8"],
  "gold_findings": [
    {"kind": "contradicted_constraint", "node_id": "decision_17", "severity": "block"},
    {"kind": "missing_evidence_risk", "node_id": "risk_5", "severity": "revise"}
  ]
}
```

### Proposed Agentic Decision Case

```json
{
  "case_id": "agent_change_storage",
  "proposed_action": "Change persistence from Postgres tables to a graph database adapter.",
  "actor_id": "agent:codex:session_123",
  "snapshot_id": "org_demo_2026_04_01",
  "gold_outcome": "escalate",
  "gold_required_checks": ["decision_17", "rejected_option_2", "assumption_3"]
}
```

### Context-Summary Tool Trace

```json
{
  "trace_id": "trace_001",
  "case_id": "prompt_move_to_neo4j",
  "method": "graph_plugin_context_summary",
  "tool_name": "hivemind.related_context",
  "query_text": "What prior decisions constrain moving the decision graph to Neo4j?",
  "started_at": "2026-04-10T12:00:00Z",
  "completed_at": "2026-04-10T12:00:01Z",
  "status": "ok",
  "returned_summary": "Postgres was selected because...",
  "cited_node_ids": ["decision_17", "evidence_8", "rejected_option_2"],
  "unavailable_reason": null
}
```

If the tool is unavailable, `status` is `unavailable`, `unavailable_reason` is
required, and downstream scoring must not silently treat the method as if it had
access to HiveMind context.

### Decision-Support Packet

```json
{
  "packet_id": "packet_001",
  "case_id": "agent_change_storage",
  "method": "graph_plugin_context_summary",
  "recommendation": "escalate",
  "summary": "The proposed action conflicts with decision_17...",
  "cited_node_ids": ["decision_17", "evidence_8"],
  "risks": [{"text": "Breaks agreed local-first storage direction", "node_id": "risk_5"}],
  "assumptions": [{"text": "Graph traversal latency remains acceptable", "node_id": "assumption_3"}],
  "missing_evidence": ["No benchmark showing Neo4j improves decision lookup"],
  "future_leak_nodes": []
}
```

### Run Config And Result

```json
{
  "run_id": "run_2026_04_10_smoke",
  "suite_version": "decision_graph_v1",
  "phase": "phase_1_discovery",
  "method": "hybrid",
  "dataset_id": "smoke_v1",
  "snapshot_id": "org_demo_2026_03_01",
  "offline": true,
  "provider": null,
  "model": null,
  "started_at": "2026-04-10T12:00:00Z",
  "ended_at": "2026-04-10T12:00:02Z",
  "latency_ms": 2000,
  "token_cost": {"input_tokens": 0, "output_tokens": 0, "estimated_usd": 0.0},
  "metrics": {"recall_at_5": 0.8, "mrr": 0.75}
}
```

## Metric Definitions

Let `G` be the set of gold node IDs for a case and `R_k` be the first `k`
returned node IDs.

- Recall@K = `|G intersect R_k| / |G|`.
- MRR = mean over cases of `1 / rank_first_relevant`, or `0` if no relevant
  node is returned.
- DCG@K = `sum_i((2^gain_i - 1) / log2(i + 1))` for ranks `i` from `1` to
  `K`; nDCG@K = `DCG@K / ideal_DCG@K`.
- Latency = wall-clock milliseconds from method invocation start to final
  structured result, excluding dataset load unless explicitly reported as
  `load_latency_ms`.
- Token cost = provider-reported tokens and estimated USD when a model is used;
  deterministic offline methods report zero tokens and zero USD.

Path metrics:

- Path recall = `|gold_path_nodes intersect predicted_path_nodes| / |gold_path_nodes|`.
- Edge accuracy = exact directed edge matches divided by gold directed edges.
- Temporal ordering accuracy = fraction of comparable gold node pairs whose
  predicted order matches the gold chronological order.
- Contradiction detection = F1 over gold contradiction pairs, where a pair
  matches only when both old and new node IDs are identified.

Verification metrics:

- Prompt verification accuracy = exact match or configured partial-credit score
  for required implications: `proceed`, `revise`, `block`, or `escalate`.
- Agentic-decision verification accuracy = same implication score for proposed
  agent actions, plus required-check recall.
- Relevant-decision recall = required prior decision nodes found divided by
  required prior decision nodes.
- Evidence grounding = supported findings with cited evidence nodes divided by
  all findings that require evidence.
- Stale-assumption detection = stale or refuted gold assumptions found divided
  by stale or refuted gold assumptions.
- Contradicted-constraint detection = contradicted gold constraints found
  divided by contradicted gold constraints.
- Missing-evidence risk recall = gold missing-evidence risks found divided by
  gold missing-evidence risks.
- False-positive burden = non-gold severe findings divided by all severe
  findings; reports should show this alongside recall.

Context-summary metrics:

- Context-summary grounding = claims in the returned summary that are supported
  by cited nodes divided by total checkable claims.
- Citation accuracy = cited node IDs that support the adjacent claim divided by
  cited node IDs used.
- Required-citation recall = required gold nodes cited divided by required gold
  nodes.
- Unavailable-tool correctness = unavailable tool runs return explicit
  unavailable status and do not fabricate graph context.

Decision-support packet quality:

Use a deterministic rubric for smoke tests and an optional judged path for full
runs. The initial deterministic score is the weighted mean of:

- outcome implication correctness: 30 percent
- relevant decision/evidence recall: 25 percent
- risk and assumption recall: 20 percent
- citation/grounding quality: 15 percent
- concise actionability for a human decision-maker: 10 percent

Optional judged runs may use an LLM or human adjudication, but must record the
judge identity, prompt, model/provider when applicable, and score rationale.

Future-leak prevention:

- A run has a hard failure if any returned node, citation, summary claim, or
  packet finding depends on a node with `created_at > cutoff_time`.
- Future-leak rate = cases with at least one leak divided by total cases.

## Runner Behavior

The runner should support this command shape:

```bash
decision-graph-bench run \
  --phase phase_1_discovery \
  --method hybrid \
  --dataset benchmarks/decision_graph/fixtures/smoke/discovery.jsonl \
  --snapshot benchmarks/decision_graph/fixtures/smoke/snapshot.json \
  --output /tmp/hivemind-bench-results.jsonl \
  --offline
```

Required behavior:

- Offline smoke mode uses only local fixtures and deterministic adapters.
- Optional vector and LLM methods require explicit configuration, for example
  `HIVEMIND_BENCH_ENABLE_MODEL=1` and a provider config file.
- Optional methods that are not configured emit `status: unavailable` result
  records; they do not fail offline CI.
- Every result records method condition, dataset version, graph snapshot, runner
  version, latency, token cost, and unavailable reason when applicable.
- Reports compare no-context, retrieval-only, graph traversal, and graph/plugin
  summary paths side by side.

## Agent Tool-Call Verification

For prompt verification, agentic-decision verification, and decision-support
replay, the benchmark must prove whether the agent actually asked HiveMind for
related graph context.

Required trace fields:

- `tool_name`
- `query_text`
- `started_at`
- `completed_at`
- `status`
- `returned_summary`
- `cited_node_ids`
- `unavailable_reason`

Scoring rules:

- A method labeled `graph_plugin_context_summary` receives zero
  context-summary credit if no tool-call trace is present.
- A summary claim without a cited node receives no grounding credit.
- A cited node that does not support its adjacent claim counts against citation
  accuracy.
- Tool failure is acceptable only when represented explicitly as unavailable
  and when the final packet does not claim unavailable graph context.

## Follow-Up Implementation Beads

The existing implementation DAG should remain:

- `.2`: scaffold this folder, schemas, fixtures, and smoke runner shape.
- `.3`: create Phase 1 discovery dataset and gold-node validator.
- `.4`: implement Phase 1 retrieval metrics harness.
- `.5`: implement Phase 2 graph path benchmark and scorer.
- `.6`: implement Phase 3 decision-support replay benchmark.
- `.9`: implement Phase 4 prompt and agentic-decision verification benchmark.
- `.10`: implement Phase 5 graph/plugin context-summary benchmark.
- `.7`: generate reports for with-vs-without HiveMind demos.
- `.8`: document benchmark operation and add deterministic CI smoke coverage.

## Risks And Open Questions

- Gold labels can encode author bias. Dataset docs must explain who labeled each
  case and what coverage gaps remain.
- Optional vector/LLM methods can drift by model version. Results must record
  provider, model, embedding version, prompt, and cost assumptions.
- Graph/plugin summary scoring needs claim segmentation. The smoke path may use
  hand-authored claims before a richer checker exists.
- Decision-support packet quality is partly subjective. The deterministic smoke
  rubric is required; optional judged runs are evidence, not a replacement for
  deterministic metrics.
- HiveMind should avoid claims that it autonomously improves decisions. Reports
  must phrase gains as improved discovery, verification, grounding,
  summarization, and support-packet quality.
