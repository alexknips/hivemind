# Decision Event Granularity Study

Status: research recommendation for `hivemind-decision-event-granularity-study-1llpf`.

Question: should HiveMind clients emit options, evidence, hypotheses, rationale,
and decisions as separate low-level events, as one compound decision command, or
both?

## Recommendation

Support both at the API boundary, but keep storage canonical and decomposed.

- Canonical storage events stay node/edge shaped: `evidence.recorded`,
  `hypothesis.recorded`, `option.recorded`, `decision.proposed`,
  `decision.accepted`, `decision.rejected`, `decision.superseded`, and
  `relation.added`.
- Human and agent clients get compound convenience commands:
  `decision.capture` or `decision.propose` may include options, chosen option,
  rationale, evidence, assumptions, source refs, and immediate follow-up effects.
- The service decomposes compound commands inside one transaction into canonical
  events and projected graph nodes/edges.
- Evidence, hypotheses, and options can still be emitted independently before or
  after a decision, because public decision workflows often discover them out of
  order.
- Options should be both separately addressable and bundle-friendly. A CLI/UI can
  capture options inline, but each option should become a canonical `Option`
  node so it can later be rejected, superseded, evidenced, or reused.

The ergonomics layer is compound; the ledger is decomposed.

## Source Catalog

Human decision-document sources:

- H1: ADR GitHub organization, "Architecture decision record" -
  https://github.com/architecture-decision-record/architecture-decision-record
- H2: MADR project and template -
  https://adr.github.io/madr/
- H3: Microsoft Azure Well-Architected ADR guidance -
  https://learn.microsoft.com/en-ie/azure/well-architected/architect-role/architecture-decision-record
- H4: AWS Prescriptive Guidance ADR process -
  https://docs.aws.amazon.com/prescriptive-guidance/latest/architectural-decision-records/adr-process.html
- H5: Rust RFC template -
  https://raw.githubusercontent.com/rust-lang/rfcs/master/0000-template.md
- H6: Kubernetes KEP process and template -
  https://github.com/kubernetes/enhancements/blob/master/keps/README.md and
  https://raw.githubusercontent.com/kubernetes/enhancements/master/keps/NNNN-kep-template/README.md
- H7: Python PEP process/template -
  https://peps.python.org/pep-0001/ and https://peps.python.org/pep-0012/
- H8: OpenTelemetry OTEP process -
  https://github.com/open-telemetry/oteps
- H9: devx ADR template gist -
  https://gist.github.com/devx/87f79e59141e9bb931fab43d09db63db
- H10: klokwrk ADR example -
  https://github.com/croz-ltd/klokwrk-project/blob/master/support/documentation/adr/content/0001-architectural-decision-records.md

Chat, agent, and LLM workflow sources:

- A1: Claude Code memory docs -
  https://docs.claude.com/en/docs/claude-code/memory
- A2: Claude Code hooks reference -
  https://code.claude.com/docs/en/hooks
- A3: OpenAI Agents SDK overview -
  https://platform.openai.com/docs/guides/agents-sdk/
- A4: OpenAI Agents SDK sessions -
  https://openai.github.io/openai-agents-python/sessions/
- A5: Codex repository `AGENTS.md` -
  https://raw.githubusercontent.com/openai/codex/main/AGENTS.md
- A6: Codex advanced/MCP docs -
  https://raw.githubusercontent.com/openai/codex/main/docs/advanced.md
- A7: AutoGen AgentChat memory docs -
  https://www.mintlify.com/microsoft/autogen/agentchat/memory
- A8: Letta context hierarchy docs -
  https://docs.letta.com/guides/core-concepts/memory/context-hierarchy/
- A9: CrewAI memory docs -
  https://docs.crewai.com/en/concepts/memory
- A10: MemoryGraph MCP memory server -
  https://github.com/memory-graph/memory-graph
- A11: codesight knowledge mode -
  https://github.com/Houseofmvps/codesight
- A12: PAI-OpenCode personal AI infrastructure -
  https://github.com/Steffen025/pai-opencode
- A13: Claude Code project-manager skill gist -
  https://gist.github.com/TiuTalk/51bcb67545469bb7deaf1655b86ef2ef
- A14: Startup OS Claude Code skills, ADR lifecycle -
  https://github.com/ncklrs/startup-os-skills

## Combination Matrix

| Combination | Human examples | Agent examples | Observed emission shape | HiveMind implication |
| --- | --- | --- | --- | --- |
| Evidence or hypothesis before decision | H2, H5, H6, H7 | A1, A7, A8, A10 | Motivation, context, prior art, experience reports, or memory facts appear before the proposal/decision. Agents similarly load project memory, query memory, or recall graph memories before action. | Keep independent `Evidence` and `Hypothesis` emits. Compound decision capture may create them inline, but the API must also let actors record them before the decision exists. |
| Options or alternatives before decision | H2, H5, H6, H9, H10 | A10, A13, A14 | MADR/ADR/RFC/KEP artifacts enumerate considered options before the outcome. Agent workflows ask for options, conflicts, stakeholder positions, and decision readiness before implementation. | Add canonical `option.recorded` instead of treating options as plain strings forever. Inline options should be decomposed to nodes in one transaction. |
| Decision plus selected option and alternatives bundled together | H2, H3, H9, H10 | A11, A13, A14 | ADR-style records put context, options, selected option, and outcome in one artifact. codesight extracts decision records from docs; skills produce a structured decision/requirements artifact. | Offer `decision.capture` for human ergonomics. The service should fan out to `Option` nodes, `HAS_OPTION`, `CHOSE`, and `decision.proposed`. |
| Decision, rationale, evidence, and consequences bundled together | H1, H3, H4, H5, H8, H9 | A5, A10, A11, A12 | ADR/RFC/OTEP/KEP records keep rationale and consequences with the decision. Agent instruction and memory systems store rules, decisions, rationale, accomplishments, and consequences as reusable context. | The compound command should accept `rationale`, `evidence[]`, `consequences[]`, and source refs, but projected graph state must keep evidence addressable and consequences queryable. |
| Hypotheses/options revised, rejected, or superseded separately | H3, H4, H7, H8 | A2, A7, A8, A10 | ADR states include proposed/accepted/superseded; AWS says accepted ADRs are immutable and later insights create a superseding ADR; PEPs record rejected ideas; OTEPs preserve rejected/withdrawn PRs. Agent systems emit hook/session/memory events that can update, compact, or supersede context independently. | Treat disagreement and supersession as additive events. Add `option.rejected` or represent it with `relation.added` from actor to option; add first-class revision/supersession paths for hypotheses/options if product queries need them. |
| Follow-up tasks, consequences, or implementation events after decision | H4, H6, H8, H9 | A2, A6, A10, A12 | AWS ADR review creates action points; KEPs track implementation history and release checklist; OTEPs create implementation issues after approval. Agents emit lifecycle hooks, MCP event streams, memory relationships, ratings, and learning loops after work. | Keep follow-ups out of decision status. Model them as consequences/evidence/tasks in adjacent systems, or later add `CONSEQUENCE_OF`/`REALIZES` edges without turning HiveMind into a task tracker. |
| Actor/source/provenance as node property vs envelope metadata | H4, H6, H7, H9, H10 | A2, A3, A4, A5, A10 | Human processes put authors/reviewers/stakeholders in headers or metadata. Agent frameworks put session id, transcript path, cwd, agent id, run state, tool call, or source file in envelopes. | Store actor, org, event uuid, correlation, causation, timestamp, and source refs in the event envelope. Project actor/source nodes and `event_origin` into the graph for queryability. Do not bury provenance only inside payload text. |
| Knowledge-map extraction from existing artifacts | H1, H2, H10 | A1, A10, A11 | Existing docs are scanned later to recover decisions and context. Agent tools generate compact knowledge maps from ADRs, meeting notes, retros, and markdown memory. | Provide importers that decompose legacy docs into canonical events with source refs. Mark imported events as imported/provisional if the original actor/time cannot be proven. |

No material search gap remained after expanding "chat/agent examples" to include
public workflow docs, memory systems, prompts, and tool docs, which the bead
explicitly permits.

## Worked HiveMind Example

Input scenario:

```bash
cargo run -- --actor alice emit evidence.recorded \
  --content "SQLite WAL is sufficient for slice-1 local writes"

cargo run -- --actor alice emit hypothesis.recorded \
  --statement "Embedded storage keeps onboarding under five minutes"

cargo run -- --actor alice emit decision.proposed \
  --title "Use embedded storage for slice 1" \
  --rationale "It keeps the prototype single-process and easy to replay" \
  --topic-keys architecture,storage \
  --options sqlite,postgres \
  --chose sqlite
```

### Encoding A: Fully Separated Emits

This is explicit and replay-friendly:

```text
1. evidence.recorded
   payload: { evidence_id, content, source? }

2. hypothesis.recorded
   payload: { hypothesis_id, statement }

3. option.recorded
   payload: { option_id: "sqlite", label: "SQLite/Kuzu embedded" }

4. option.recorded
   payload: { option_id: "postgres", label: "Postgres remote" }

5. decision.proposed
   payload: {
     decision_id,
     title,
     rationale,
     topic_keys: ["architecture", "storage"]
   }

6. relation.added Decision -BASED_ON-> Evidence
7. relation.added Decision -ASSUMES-> Hypothesis
8. relation.added Decision -HAS_OPTION-> sqlite
9. relation.added Decision -HAS_OPTION-> postgres
10. relation.added Decision -CHOSE-> sqlite
```

Pros:

- Each node can be created, linked, revised, and queried independently.
- Idempotency is simple per event UUID.
- Replay is deterministic and failure can resume from the last committed event.
- Provenance can differ per node if Alice gathered evidence and Bob proposed the
  decision later.

Cons:

- It is too verbose for common human capture and too brittle for chat agents.
- Partial capture is easy if a client crashes between emits.
- Users must understand graph structure before they can record a decision.

### Encoding B: Compound Command, Canonical Decomposition

Client-facing command:

```json
{
  "type": "decision.capture",
  "actor_id": "alice",
  "client_request_id": "req-123",
  "decision": {
    "title": "Use embedded storage for slice 1",
    "rationale": "It keeps the prototype single-process and easy to replay",
    "topic_keys": ["architecture", "storage"]
  },
  "evidence": [
    {
      "content": "SQLite WAL is sufficient for slice-1 local writes",
      "source": "cli"
    }
  ],
  "hypotheses": [
    {
      "statement": "Embedded storage keeps onboarding under five minutes"
    }
  ],
  "options": [
    { "label": "sqlite" },
    { "label": "postgres" }
  ],
  "chosen_option": "sqlite"
}
```

Service transaction:

```text
begin transaction
  validate actor and topic keys
  append command.accepted/correlation root, or assign correlation id
  append evidence.recorded
  append hypothesis.recorded
  append option.recorded sqlite
  append option.recorded postgres
  append decision.proposed
  append relation.added edges
  project all nodes and edges with event_origin
commit
```

The caller sees one successful capture. The ledger still contains the decomposed
canonical events needed for audit and graph queries.

Pros:

- One ergonomic operation for humans, Slack capture, and coding agents.
- One transaction avoids half-decisions.
- Canonical replay and event-origin tracing remain intact.
- The decomposition boundary is owned by the service, not by every client.

Cons:

- The service must define deterministic event ordering inside the transaction.
- Idempotency must work at both the compound request and individual event level.
- Validation errors need precise paths, for example `options[1].label`.

## Design Comparison

| Concern | Fully separated emits | Compound command only | Compound command decomposed to canonical events |
| --- | --- | --- | --- |
| Transaction boundary | Many small commits unless client batches manually. | One commit, but storage event is too coarse. | One service transaction with many canonical events. |
| Idempotency | Simple per event; difficult across multi-step client workflows. | Simple per command; hard to replay/query internals. | Use `client_request_id` for command and `event_uuid` per generated event. |
| Partial failure | Common if client crashes between emits. | Rare, but failed validation can obscure which part is bad. | Rare; service can validate all payload paths before append. |
| Replay | Excellent. | Weak unless command replay reimplements decomposition forever. | Excellent if generated events are canonical. |
| Provenance | Best when different actors provide different pieces. | Poor if all facts inherit one actor/source. | Good: default command actor, with per-item source/actor overrides when needed. |
| Queryability | Best. | Poor unless every query parses nested payloads. | Best: graph nodes and edges are projected directly. |
| Human ergonomics | Poor for first use. | Best. | Best at the boundary; rigorous underneath. |
| Agent ergonomics | Brittle across long tool sessions. | Good for capture, poor for later targeted updates. | Best: agents can use compound capture or precise low-level updates. |

## API Shape

Recommended commands:

```text
evidence.recorded
hypothesis.recorded
option.recorded
decision.proposed
decision.accepted
decision.rejected
decision.superseded
relation.added
decision.capture        # convenience command; service decomposes
```

Recommended event envelope fields:

```text
org_id
event_id
event_uuid
client_request_id       # optional idempotency key for compound commands
correlation_id
causation_event_id
actor_id
type
payload
source_ref              # optional: URL, transcript path, Slack ts, file path
ts
schema_version
signature               # later
```

Recommended relation creation rules:

- `decision.capture` may create and link all included options, evidence, and
  hypotheses.
- Low-level commands may create nodes first and attach them later.
- Every projected node/edge stores `event_origin`.
- If a compound item references an existing node by id, the service links it
  instead of duplicating it.
- If a compound item has no id, the service generates a deterministic id only
  within the transaction result, not from mutable text alone.

## Follow-Up Implementation Beads

1. `feature/P0`: Add `option.recorded` event schema, command API, projection,
   and tests; keep existing CLI inline options as compatibility sugar.
2. `feature/P0`: Add `decision.capture` command that decomposes evidence,
   hypotheses, options, chosen option, and decision into canonical events in one
   transaction.
3. `feature/P0`: Add compound command idempotency with `client_request_id` and
   deterministic generated-event correlation.
4. `feature/P1`: Add source reference fields for CLI, Slack, transcript, file,
   and imported-doc provenance.
5. `feature/P1`: Add import mode for existing ADR/MADR/PEP-like markdown that
   creates provisional canonical events with source refs.
6. `task/P1`: Decide whether rejected/superseded options and hypotheses need
   first-class event types or can remain relation/status derivations.
7. `task/P1`: Add query fixtures for bundled capture, separated capture, mixed
   actor provenance, and partial validation failures.

## Search Queries Run

- `Architecture Decision Records Context Decision Status Consequences public template`
- `MADR architecture decision record template considered options pros cons decision outcome`
- `Rust RFC template motivation rationale alternatives unresolved questions`
- `Kubernetes KEP template alternatives risks implementation history`
- `Python PEP template rationale rejected ideas open issues official`
- `OpenTelemetry OTEP template alternatives tradeoffs future possibilities github`
- `Claude Code memory docs CLAUDE.md public agent decisions rationale`
- `Claude Code hooks docs transcript_path session_id JSON public`
- `LangGraph memory docs semantic episodic procedural memory public`
- `AutoGen AgentChat memory ListMemory MemoryContent docs public`
- `Letta memory blocks archival memory docs agent memory public`
- `CrewAI memory docs short term long term entity memory public`
- `OpenAI Agents SDK sessions docs items tracing handoffs public`
- `Codex CLI AGENTS.md docs instructions public`
- Seed source URLs listed in the bead were also opened and reviewed directly.

## Verification Commands

- `gc prime`
- `GC_BEADS=bd gc bd show hivemind-decision-event-granularity-study-1llpf`
- `gc formula show mol-polecat-work`
- `cargo test`
