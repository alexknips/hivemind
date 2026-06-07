# Capture Classifier Design

Status: design for bead `hivemind-m1-5-hidden-capture-jtkmje.3`.

This document specifies the tiny classifier subagent used by the unified
`/capture` flow. The classifier reads a small text-only batch of recent agent
activity and returns capture candidates, or an empty result. It is a layer-3
suggestion component: it does not write ledger events, query the graph, rank
existing decisions, deduplicate, or resolve disagreement.

The canonical write path remains the commands layer. The unified `/capture`
implementation may call this classifier when `--kind` is omitted, then dispatch
the selected capture to the existing explicit HiveMind emit command.

## Model Choice

Use Claude Haiku 4.5 as the default classifier model. Prefer the pinned API
model id `claude-haiku-4-5-20251001`; allow an operator override, but do not
build multi-model routing into the first implementation.

As of 2026-06-06, Anthropic's model overview lists the current production
ladder as Opus 4.8, Sonnet 4.6, and Haiku 4.5. The same page lists Haiku 4.5 as
the fastest current model, with near-frontier intelligence, a 200k context
window, 64k max output, and $1 / $5 per million input/output tokens. It lists
Sonnet 4.6 as the stronger speed-plus-intelligence tier with 1M context and
$3 / $15 pricing.

Haiku 4.5 is the right first tier because the classifier task is small,
latency-sensitive, mostly qualitative, and conservative misses are acceptable.
The prompt asks for a handful of node-kind decisions over 3 to 5 recent turns,
not long-horizon planning or full-transcript reasoning. Sonnet 4.6 is the
fallback choice if real-session tuning shows Haiku missing durable decisions
that humans and Sonnet consistently catch, but that should be proven with false
negative samples before paying the extra latency and cost.

Use Claude structured outputs for the API-backed implementation. Anthropic
documents structured outputs as generally available for Haiku 4.5, and the
feature guarantees valid JSON against a supplied schema in ordinary non-refusal,
non-token-limit cases.

References:

- Anthropic models overview:
  https://platform.claude.com/docs/en/about-claude/models/overview
- Claude Haiku 4.5 model page:
  https://www.anthropic.com/claude/haiku
- Claude structured outputs:
  https://platform.claude.com/docs/en/build-with-claude/structured-outputs
- Claude Sonnet 4.6 announcement:
  https://www.anthropic.com/news/claude-sonnet-4-6

## Input

The classifier input is a batch of recent session turns, rendered as text. Do
not include raw tool-call payloads, full command output, file diffs, screenshots,
or binary/media content. If the caller summarizes a tool result, label it as a
summary and keep it short.

Recommended batch envelope:

```json
{
  "batch_id": "session-id:offset-range",
  "agent_tool": "claude|codex|other",
  "session_id": "...",
  "turns": [
    {
      "turn_id": "...",
      "role": "user|assistant|system|tool-summary",
      "text": "...",
      "truncated": false
    }
  ]
}
```

Start with the most recent 3 to 5 turns. Four turns is the default. The Stop
hook should submit the latest small batch in the background after the agent
finishes a turn. The user's next turn must not wait for this classifier.

Truncation must be explicit. If a turn is shortened before classification, set
`truncated: true` and include a clear marker in the text. Do not silently trim a
batch and present it as complete.

## Batching Parameters

- Batch size: start with 4 recent turns; permit 3 to 5 through configuration.
- Input content: text only, no raw tool payloads.
- Per-turn budget: keep each turn concise enough that the whole request remains
  comfortably below 8k input tokens.
- Output budget: cap at roughly 1200 tokens; most valid outputs should be under
  300 tokens.
- Sampling: low temperature, no extended thinking.
- Latency target: warmed p95 under 2 seconds per batch on Haiku 4.5.
- Timeout: if the background classifier exceeds 5 seconds, drop the attempt and
  log the timeout for later tuning.
- Retry policy: no synchronous retry from the Stop hook. Retry only in offline
  evaluation tooling.

Structured-output schema compilation may add first-use latency. The 2 second
target applies to the warmed schema path.

## Output Schema

The classifier returns exactly one JSON object:

```json
{
  "captures": [
    {
      "kind": "decision",
      "title": "Use Haiku 4.5 for capture classification",
      "rationale": "The task is small, latency-sensitive, and tolerant of conservative misses.",
      "topic_keys": ["capture", "classifier", "agents"],
      "evidence_ids": [],
      "options": ["haiku", "sonnet"],
      "chosen_option": "haiku",
      "confidence": 0.86
    }
  ]
}
```

`captures` may be empty, and an empty array is the expected result for most
batches. There is no `"none"` kind.

Allowed `kind` values:

- `decision`
- `evidence`
- `hypothesis`
- `blocker`
- `decision-request`
- `notification`

Field rules:

- `title`: concise, durable label for the thing worth capturing.
- `rationale`: why this should become organizational memory, not a transcript
  summary.
- `topic_keys`: 1 to 5 lowercase search keys.
- `evidence_ids`: only IDs already present in the input batch; never invent
  graph IDs.
- `options`: for decisions, list the meaningful alternatives if visible;
  otherwise `null`.
- `chosen_option`: for decisions, the chosen path if visible; otherwise `null`.
- `confidence`: classifier self-estimate from 0.0 to 1.0 for offline prompt
  tuning. It is not graph provenance, not a query ranking score, and must not be
  presented as authoritative confidence unless stored with the classifier model,
  prompt version, schema version, and source batch id.

For API structured outputs, use a JSON Schema equivalent to the object above
with `additionalProperties: false`. Keep all fields required; nullable values
should be represented explicitly.

## Kind Guidelines

Capture a `decision` when the batch contains a chosen path among plausible
alternatives. It may be proposed rather than accepted, but it must have a
selection and a reason. If the batch only asks someone to choose later, use
`decision-request`.

Capture `evidence` when the batch contains an observation with a referent that
can support or refute later choices: a test result, production symptom, external
document fact, measured latency, user research finding, or verified constraint.
Do not capture raw command output unless the observation has been interpreted.

Capture a `hypothesis` when the batch states a proposition that is being tested
or may later be supported/refuted. Prefer hypothesis over evidence when the
claim is uncertain.

Capture a `blocker` when progress is materially blocked by a dependency,
permission issue, missing artifact, failing gate, unavailable service, or
unresolved external decision. Routine "I am still working" status is not a
blocker.

Capture a `decision-request` when the durable value is that an actor needs a
decision from another actor, and no chosen path is visible yet.

Capture a `notification` only for durable announcements another actor would
need after session restart: handoff state, merge-ready status, rejected
integration with proof, or a completed verification result. Do not capture
ordinary gc/br claim mechanics.

Do not capture:

- synthetic seed data, fake demo decisions, or fixture content unless the user
  is explicitly designing the fixture itself;
- gc/br plumbing chatter, branch naming, routine claim/drain/merge mechanics, or
  gate output with no product implication;
- raw tool-result noise, stack traces, logs, or diffs without a summarized
  observation;
- private scratch notes, plans, TODO lists, or task tracking;
- repeated statements already captured in the same batch;
- borderline material. Omit it and tune later if humans identify a real miss.

## Classifier Prompt

Drop this prompt into `.claude/agents/hivemind-classifier.md` for the first
implementation, with the output schema enforced by structured outputs where
available.

```markdown
You are the HiveMind capture classifier.

HiveMind stores organizational decision memory: durable decisions, evidence,
hypotheses, blockers, decision requests, and notifications with provenance. It
does not store chat history, task tracking, private scratch notes, or raw tool
logs.

Read the batch of recent agent activity. Return only JSON matching the capture
schema. Most batches should return {"captures":[]}.

Capture a decision only when the text shows a chosen path among plausible
alternatives and gives or implies a reason. If a choice is requested but not yet
made, use decision-request instead.

Capture evidence only when there is an observation with a referent that could
support or refute a later decision, such as a test result, production symptom,
verified external fact, measured latency, or explicit user research finding.

Capture a hypothesis only when the text states a proposition being tested or a
claim that may later be supported or refuted.

Capture a blocker only when progress is materially stopped by a dependency,
permission issue, unavailable service, missing artifact, failing gate, or
unresolved external decision.

Capture a notification only when another actor would need the announcement
after restart, such as handoff state, merge readiness with proof, rejection
state, or completed verification.

Do not capture synthetic test data, fixture/demo content, gc/br plumbing,
branch-name mechanics, routine gate chatter, raw command output, stack traces,
file diffs, TODO lists, status narration, or generic plans. If the material is
borderline, omit it.

Never invent evidence ids. Use only ids present in the input. Keep titles short.
Use 1 to 5 lowercase topic keys. Confidence is your self-estimate for offline
tuning, not authoritative truth.
```

## Evaluation And Tuning

Tune against real session logs after the implementation bead lands. Review false
positives first because noisy capture damages trust faster than conservative
omission. Then review false negatives for durable design decisions missed by
Haiku.

Keep the first tuning loop prompt-only. Do not add hard thresholds, similarity
dedupe, graph reads, or write-path inference to compensate for classifier
mistakes. If the classifier is too noisy, tighten the prompt and examples. If it
misses real decisions that Sonnet catches consistently, record that evidence and
consider a separate bead to revisit the model tier.

## Dogfood Recording

This design choice should be captured as a `decision.proposed` event in the rig
ledger after the fragmentation fix writes agent captures to
`/data/projects/hivemind/hivemind/ledger.sqlite` and the unified `/capture`
command exists. The current checkout still has only `/capture-decision`, so
this design bead records the choice in stable docs and leaves the ledger
dogfood step to the first bead where the intended capture path exists.
