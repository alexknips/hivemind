# Local Text Import And Decision Diff Semantics

This document defines the first local Milestone 2 semantics for importing
decisions from text files and asking which decision nodes were added to the
graph since a time boundary such as "last week".

The scope is local-first. It does not define a hosted ingestion service, a
general document knowledge graph, or an LLM extraction pipeline.

## First Slice

The first import slice accepts UTF-8 local files in these forms:

- Markdown files (`.md`, `.markdown`).
- Plain text files (`.txt`).
- Markdown or plain text files containing explicit HiveMind decision blocks.
- Prepared PDF/OCR text emitted by `hivemind import prepare-documents`.

Only explicit decision blocks create decision events in the first slice. An
unmarked document is reported as `skipped_unmarked`; it is not heuristically
converted into decisions. This keeps extraction deterministic and prevents a
local importer from inventing organizational memory.

A decision block is a structured section inside a text file. The exact parser
can evolve, but the first implementation should support a compact YAML-like
shape with these fields:

```text
Decision:
  id: local-stable-id
  title: Use SQLite for the local prototype
  status: accepted
  actor: alice
  topic_keys: storage,local
  rationale: Keeps onboarding under five minutes.
  options:
    - sqlite
    - postgres
  chose: sqlite
  evidence:
    - Local replay tests complete in under one second.
  hypotheses:
    - Embedded storage is enough for single-user slice 1.
  supersedes:
    - decision-old-storage-plan
```

`id`, `title`, `status`, and `rationale` are required for a decision import.
`actor` is required when the source document identifies the original decision
actor. If the source does not identify an original actor, the importer records
the importer as the event actor and marks the imported decision as provisional
in import metadata.

## Provenance

Imported events must preserve both ledger provenance and document provenance.

Every generated ledger event uses:

- `actor_id`: the original actor named in the block when present; otherwise the
  importer actor.
- `source`: `document`.
- `source_ref`: a stable local locator containing the canonical path, document
  content hash, import run id, and original span.
- `event_uuid`: a deterministic UUID derived from the import idempotency key.

The importer also records an import run id in structured output and in generated
source refs. A run id should be stable for one invocation, for example
`import:2026-05-19T02:45:00Z:<short-hash>`.

Document provenance fields are:

- `source_path`: canonical local path at import time.
- `source_hash`: SHA-256 of the file bytes.
- `source_span`: byte range and, when available, line range for the decision
  block.
- `source_snippet`: short original text excerpt suitable for citation.
- `importer_actor_id`: actor running the import.
- `original_actor_id`: actor named in the source, when present.
- `import_run_id`: the invocation that produced the events.

Projection and query DTOs should surface enough of this provenance for a user to
trace an imported decision back to the source file and span. A cited UI or
history agent must cite the decision id, event origin, source ref, and source
snippet rather than paraphrasing imported text without attribution.

## Extraction Mode

Slice 1 extraction is deterministic structured-marker extraction:

1. Scan each accepted local file for explicit decision blocks.
2. Validate required fields and supported statuses.
3. Emit canonical HiveMind events for the decision, options, evidence,
   hypotheses, supersession, and relations represented in the block.
4. Report validation errors without writing partial events for that block.

LLM-assisted extraction is layer-3 suggestion work. It may propose candidate
blocks, but those candidates must be reviewed or materialized as explicit
structured input before the write layer appends ledger events. The importer must
not call an LLM from the write path.

## LLM Candidate Workflow

`hivemind suggest document-candidates` is outside the write and query paths. It
reads local UTF-8 markdown or text documents, sends source text plus provenance
to an external extractor command or consumes an already-produced LLM response
file, and returns pending-review candidates. The command does not open the
ledger and does not append events.

Each candidate includes:

- A structured decision draft: `id`, `title`, `status`, `topic_keys`,
  `rationale`, `options`, optional `chose`, `evidence`, and `hypotheses`.
- Source document provenance: canonical path, SHA-256, byte and line span, and
  source snippet.
- An extraction explanation and optional non-numeric confidence explanation.
- A materialization hint that points to the explicit review step.

The external extractor protocol is JSON over stdin/stdout when
`--extractor-command hivemind-document-extractor` is used. The executable name
is fixed so this path can be installed deliberately without allowing arbitrary
command execution from document-processing input. For offline review or tests,
`--llm-response <path>` can supply the same response JSON directly:

```bash
hivemind --json suggest document-candidates \
  --file ./decision-memo.txt \
  --llm-response ./llm-response.json > candidates.json
```

The accepted LLM response shape is:

```json
{
  "candidates": [
    {
      "file_index": 0,
      "source_span": {
        "byte_start": 0,
        "byte_end": 120,
        "line_start": 1,
        "line_end": 4
      },
      "title": "Use reviewed document extraction",
      "status": "proposed",
      "topic_keys": ["documents", "layer3"],
      "rationale": "Automatic ledger writes would bypass review.",
      "option_labels": ["review candidates first", "auto-import everything"],
      "chosen_option_label": "review candidates first",
      "evidence": ["The memo says automatic ledger writes would bypass review."],
      "hypotheses": ["Reviewers can inspect the candidate file quickly."],
      "explanation": "The excerpt names a choice and its rationale."
    }
  ]
}
```

After a human or agent reviewer accepts a candidate, materialize it to a normal
document-import block:

```bash
hivemind --actor reviewer:alice --json suggest materialize-document-candidates \
  --input candidates.json \
  --candidate-id candidate:document:abc123 \
  --output reviewed.md
```

Only a later explicit `hivemind import documents --file reviewed.md` command
writes events. This keeps deterministic import usable without any LLM workflow
installed or enabled.

## PDF And OCR Preparation

PDF and OCR inputs use a preparation step, not a direct ledger write:

```bash
hivemind --json import prepare-documents ./source.pdf --output-dir ./prepared
hivemind --actor alice --json import documents ./prepared
```

`prepare-documents` extracts text PDF pages and OCR-backed UTF-8 files such as
`scan.ocr.txt` into deterministic intermediate text. Each prepared page includes
a `# hivemind-source-ref: {...}` comment containing the original source path,
source hash, preparation run id, extraction kind, page number, page span, and a
short source snippet. The reviewed intermediate text remains ordinary document
input; `import documents` still performs the same structured block validation
before writing events.

PDFs without an extractable text layer are reported as `needs_ocr` and write no
prepared text. OCR-backed text is reported as `review_required` when confidence
metadata is unavailable, and that uncertainty is carried through the imported
event source reference as `prepared_from.ocr_review_required` and
`prepared_from.ocr_uncertainty`.

## Idempotency And Re-Import

The idempotency key for one imported decision block is:

```text
import:v1:<canonical-source-path>:<source-hash>:<block-id>:<block-span>:<event-role>
```

`event-role` distinguishes the decision event from generated option, evidence,
hypothesis, and relation events. The importer derives deterministic
`event_uuid` values from these keys so re-importing the same file content is a
no-op through existing ledger event UUID deduplication.

Stable decision ids are derived in this order:

1. If the block has an `id`, use a document namespace plus that id.
2. If no `id` exists, use source hash plus block ordinal and mark the id as
   content-derived.

Re-import behavior:

- Same source hash and same block id: no-op.
- Same block id with changed content: report a conflict with the existing
  captured decision, the proposed update, source provenance, affected graph
  dependencies, and explicit resolution actions. The default writes nothing.
- Same content under a different path: report a duplicate candidate unless the
  user explicitly imports it as a distinct source.
- Missing old block on a later import: write nothing by default. Absence in a
  document is not a decision supersession or rejection.

Conflict resolution uses explicit importer flags. `--on-conflict keep_existing`
records no events and leaves the graph unchanged. `--on-conflict supersede`
captures the proposed update as a new decision and appends a
`decision.superseded` event from the new decision to the existing decision.
`--on-conflict contest` appends a rejection event by the importing reviewer so
accepted decisions become `contested` through normal read-layer status
derivation. `--on-conflict add_context` appends proposed evidence and
hypotheses to the existing decision as new context. The importer must not infer
supersession from edited prose.

After deterministic no-op and exact duplicate checks, the importer may report
fuzzy duplicate candidates for changed prose. This advisory match compares only
explicit imported decision fields such as title, rationale, topic keys, and the
stable block id. Each match must include the matched decision id, event origin,
source reference, algorithm name, field overlap scores, and the reviewer action
required. A fuzzy match never silently merges, supersedes, or accepts a decision;
ambiguous matches write nothing until a reviewer resolves them explicitly.

## Temporal Diff

The temporal diff query returns bounded, deterministic changes in the decision
graph. The first query shape should be equivalent to:

```text
get_decisions_added_since(since, until?, filters?)
```

The query layer receives concrete bounds, not raw human phrases. Bounds may be
ledger offsets or RFC 3339 timestamps resolved by CLI/application code.

The response must include:

- `resolved_since` and `resolved_until`.
- `boundary_event_offsets` when timestamp bounds are used.
- `added_decisions`: decision nodes whose creation event origin is inside the
  window.
- `changed_existing_decisions`: existing decision nodes that gained status
  edges, evidence, hypotheses, options, supersession edges, or refuted
  assumptions inside the window.
- `filters`: source, source_ref/import_run, topic, actor, and status filters
  actually applied.
- `result_count`, `truncated`, and a continuation cursor.

A decision is "added" only when its first projected `Decision` node creation
event falls inside the window. Later acceptance, rejection, evidence, option,
hypothesis, or supersession events are changes to an existing decision, not new
decision nodes.

Offset bounds are canonical. Timestamp bounds are resolved to ledger offsets at
query time and the response includes those offsets so the same diff can be
replayed exactly.

## Relative Dates

Relative phrases are parsed outside the query layer. The query API receives
concrete timestamps or offsets.

`since last week` means:

- Choose a timezone. The default is UTC unless the caller supplies an IANA
  timezone such as `America/New_York`.
- Resolve `now` once at command start.
- Find the start of the previous ISO week, Monday 00:00:00, in that timezone.
- Use that instant as `resolved_since`.
- Use command-start `now` as `resolved_until` unless the caller supplies
  `until`.

Example with a frozen test clock:

```text
now = 2026-05-19T12:00:00Z
timezone = UTC
since last week => resolved_since = 2026-05-11T00:00:00Z
resolved_until = 2026-05-19T12:00:00Z
```

Tests must freeze `now`, pass an explicit timezone, and assert the resolved
window. UI surfaces must display the resolved window so a partial result never
looks like an implicit calendar guess.

## CLI Flow

The minimum local demo flow is:

```bash
hivemind import documents ./decision-notes \
  --actor alice \
  --format markdown \
  --json

hivemind query get_decisions_added_since \
  --since "last week" \
  --timezone UTC \
  --source document \
  --json
```

The import command writes only validated decision blocks and reports skipped
files, conflicts, duplicate candidates, generated event ids, decision ids, and
the import run id.

The diff query is read-only. It does not inspect files, re-run extraction,
deduplicate prose, or summarize with an LLM. It reads the ledger/projection
state and returns graph changes with provenance.

## Non-Goals

- Hosted document ingestion.
- Silent parsing of arbitrary prose into decisions.
- Semantic deduplication in the write path.
- Hidden supersession from edited documents.
- Treating document import as a chat transcript archive.
- Query-layer LLM calls, ranking, or summarization.
