# Ingestion Connectors — Design Document (Ingestion v1)

Status: **DRAFT — awaiting alex review before implementation begins**
Bead: hivemind-ld68  
Parent epic: hivemind-2tcl  
Author: gastown.furiosa (polecat), 2026-07-11

---

## 0. Purpose and Scope

This document designs the connector abstraction and version-walk import pipeline
for HiveMind Ingestion v1. It is a **plan-first deliverable**: no product code
beyond exploratory spikes is written until alex has reviewed and approved this
design.

Scope (v1):
- Abstract `Connector` trait with three concrete implementations: Google Docs,
  Confluence, and git-file.
- Version-walk semantics: enumerate source versions → diff adjacent versions →
  emit `decision.proposed` + `decision.superseded` chains with bitemporal
  stamps.
- Same-as/dedup layer: cross-source, re-import-aware, reversible same-as links
  with human-review gate (builds on `reviewer_action` / `DocumentReviewerAction`
  semantics already in `src/ingest.rs`).
- Invocation surfaces: CLI (`hivemind import connector ...`) and the hosted API
  (server-side import for non-technical users pasting a link).
- Relationship to the two existing lanes (decision-block import vs
  prepare+ingest/Haiku classification).

Out of scope (explicitly deferred):
- Standing watcher / continuous-poll daemon.
- Connector implementations beyond the three listed.
- Org-level connector credentials (v1 is per-user OAuth grants).

---

## 1. Connector Trait

### 1.1 Design Goals

A connector must be addable without touching the pipeline. All pipeline code
(`version_walk`, `segment_and_diff`, `same_as_resolve`, `emit_chain`) takes
`dyn Connector`, never concrete types. Adding a new connector is implementing
the trait; it is not editing the pipeline.

### 1.2 Rust Trait Signature

```rust
/// Stable identity for a source document (connector + external doc ID).
/// Opaque to the pipeline; equality is structural.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SourceId {
    pub connector_kind: ConnectorKind,
    pub doc_id: String,
}

/// A single historical version of a source document.
#[derive(Debug, Clone)]
pub struct VersionMeta {
    pub version_id: String,
    pub occurred_at: DateTime<Utc>,    // version timestamp from source
    pub author_actor: Option<String>,  // actor id when source provides authorship
    pub sequence: u64,                 // monotone ordinal (1 = oldest)
}

/// Extracted content for one version.
#[derive(Debug, Clone)]
pub struct VersionContent {
    pub version_id: String,
    pub text: String,                  // UTF-8 plain text after normalization
    pub content_hash: String,          // SHA-256 of raw source bytes
    pub source_url: Option<String>,    // canonical URL for citation
}

/// Common intermediate representation: a single statement extracted from text.
/// The pipeline segments content into statements; connectors deliver raw text.
#[derive(Debug, Clone)]
pub struct Statement {
    pub text: String,
    pub byte_span: (usize, usize),
}

#[async_trait]
pub trait Connector: Send + Sync {
    /// Human-readable connector kind (for logs, errors, provenance).
    fn kind(&self) -> ConnectorKind;

    /// Resolve a user-supplied URL or id to a SourceId.
    /// Returns None when the URL does not belong to this connector.
    async fn resolve(&self, url_or_id: &str) -> Result<Option<SourceId>>;

    /// Enumerate all versions of the document, oldest first.
    /// Implementations may page internally; callers see the full list.
    async fn list_versions(&self, source_id: &SourceId) -> Result<Vec<VersionMeta>>;

    /// Fetch the content of one version.
    async fn fetch_version(
        &self,
        source_id: &SourceId,
        version_meta: &VersionMeta,
    ) -> Result<VersionContent>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorKind {
    GoogleDocs,
    Confluence,
    GitFile,
}
```

### 1.3 Connector Implementations

**Google Docs** — `GoogleDocsConnector`
- `resolve`: accepts `docs.google.com/document/d/<id>` URLs and bare doc IDs.
  Returns `SourceId { connector_kind: GoogleDocs, doc_id: <file_id> }`.
- `list_versions`: calls Drive revisions API (`drive.revisions.list`). Returns
  revisions in `modifiedTime` order. `author_actor` is set from
  `lastModifyingUser.displayName` when present, formatted as
  `human:<display_name_slugified>`.
- `fetch_version`: calls `drive.revisions.get` with `?alt=media`, exports to
  plain text via `mimeType=text/plain`.

**Confluence** — `ConfluenceConnector`
- `resolve`: accepts Confluence page URLs (`/wiki/spaces/.../pages/<id>`) and
  bare page IDs.
- `list_versions`: calls Confluence REST API `GET /wiki/api/v2/pages/<id>/versions`.
  Returns page versions oldest-first. `author_actor` from `authorId`.
- `fetch_version`: calls `GET /wiki/rest/api/content/<id>?version=<n>&expand=body.export_view`,
  strips HTML to plain text via `ammonia`/similar.

**Git-file** — `GitFileConnector`
- `resolve`: accepts `repo_path:file_path` or a local file path within a git
  repo (detected by walking up to `.git`). Returns
  `SourceId { connector_kind: GitFile, doc_id: "<repo_root>:<file_path>" }`.
- `list_versions`: runs `git log --follow --format=%H%x00%ai%x00%an <file_path>`
  and reverses the output (oldest first). `author_actor` from git author name,
  formatted as `human:<name_slugified>`.
- `fetch_version`: runs `git show <commit>:<file_path>` and returns UTF-8 bytes.
  Confirms via `git log --format=%H -1 <commit> -- <file_path>` that the file
  existed at that revision.

### 1.4 Auth Per Connector

| Connector | Credential | Where stored | Notes |
|-----------|-----------|--------------|-------|
| Google Docs | OAuth 2.0 access + refresh token, `drive.readonly` scope | HiveMind auth sidecar, per-user grant | Initial consent via browser OAuth flow; sidecar mints a connector-scoped token. CLI first-run triggers `hivemind connector auth gdocs` which opens browser. |
| Confluence | Per-user API token (Atlassian) OR OAuth 2.0 (Confluence Cloud) | Auth sidecar, per-user grant | Atlassian Cloud supports OAuth 2.0 (3LO); Server/Data Center uses API tokens. For v1: accept a Confluence base URL + API token via env var (`HIVEMIND_CONFLUENCE_TOKEN`) for CLI. SPA flow uses OAuth. |
| Git-file | None — reads local filesystem and runs git | N/A | No credential needed; filesystem access is assumed. Remote URLs are out of scope for v1. |

**Relationship to auth sidecar** (from `docs/design/auth-requirements.md`):  
The Rust server is an OAuth *resource server*; the auth sidecar owns session
management and connector grants. For connectors, the sidecar stores per-user
OAuth tokens (access + refresh) for Google and Confluence. The connector
implementation receives a fresh access token from the sidecar before each call.
The Rust pipeline code does not handle OAuth token refresh directly.

**Calls for alex**: the connector order (which ships first), and whether v1
CLI should accept API tokens for Confluence or require OAuth. See §7 (Open
Questions).

---

## 2. Version-Walk Semantics

### 2.1 Pipeline Overview

```
list_versions()
   │
   ▼
[VersionMeta, VersionMeta, VersionMeta, ...]   (oldest → newest)
   │
   ▼  fetch_version() per version (lazy, with content-hash dedup)
   │
   ▼
segment_into_statements()   (per version text)
   │
   ▼
diff_adjacent_versions()    (pair: (v_n, v_{n+1}))
   │
   ▼
emit_event_chain()          (one or more ledger events per diff)
```

### 2.2 Statement Segmentation

Per `docs/DECISION_EVENT_GRANULARITY_STUDY.md`: the pipeline emits events at
statement granularity, not whole-document granularity. A "statement" is a
semantically self-contained clause (typically a sentence or a labeled section
heading + body pair).

Segmentation rules for v1:
1. Split on sentence boundaries (period/exclamation/question followed by
   whitespace + capital, or blank line).
2. For markdown: each heading + the text until the next same-or-higher heading
   forms one statement block.
3. Statements shorter than 20 characters are merged with the preceding
   statement.
4. Maximum statement length: 2000 characters (longer blocks are kept as-is,
   not split further in v1).

This is deterministic and requires no LLM. Layer-3 may later offer smarter
segmentation, but the write path must not call an LLM.

### 2.3 Adjacent-Version Diff

For each consecutive pair `(v_n, v_{n+1})`:

1. Compute Myers diff on the statement lists.
2. Classify each changed statement as: `Added`, `Removed`, `Modified`
   (both removed and added with similarity > 70% token overlap),
   or `Unchanged`.
3. For `Modified`: find the best-matching removed statement using the
   same `token_overlap_percent` algorithm from `find_document_similarity_matches`.
4. Skip pairs where `content_hash(v_n) == content_hash(v_{n+1})` — no-op
   (connector may return identical content under different revision IDs).

### 2.4 Event Chain Emission

**Bitemporal stamping** (invariant throughout):
- `occurred_at` = `VersionMeta.occurred_at` (the version's source timestamp).
- `recorded_at` = import-time wall clock (when we append to the ledger).

**Per-version first import** (v_n is new to the ledger):
- Each added/present statement → `decision.proposed` with:
  - `actor_id` = `VersionMeta.author_actor` when available; otherwise the
    importer actor marked provisional.
  - `occurred_at` = version timestamp.
  - `source_ref` = `ConnectorSourceRef { connector_kind, doc_id, version_id,
    content_hash, statement_span }`.
  - `import_run_id` from the invocation.

**Cross-version diff** (v_n already imported, v_{n+1} is new):
- `Added` statement → `decision.proposed` for v_{n+1}.
- `Removed` statement → `decision.superseded` pointing from v_{n+1} import to
  the existing v_n decision node, with `supersedes_id` set.
- `Modified` statement → `decision.proposed` for v_{n+1} + `decision.superseded`
  with `supersedes_id` = best-matching v_n decision. The modification is
  recorded as a supersession, not a mutation.
- `Unchanged` statement → no new event; existing decision node for v_n carries
  forward.

**Idempotency key** for one connector-version-statement event:
```
connector-import:v1:<connector_kind>:<doc_id>:<version_id>:<statement_ordinal>:<event_role>
```
Deterministic `event_uuid` is derived from this key. Re-importing the same
version with the same content is a no-op through existing ledger UUID
deduplication.

**Depth control**: the `--max-versions N` flag limits how many historical
versions to walk. Default is unlimited (walk all). The hosted API may impose
a configurable per-tenant cap.

---

## 3. Same-As / Dedup Layer

### 3.1 Why It's First-Class

From the epic: deduplication across connectors and re-imports is "very
important". The same statement may appear in a Google Doc, a Confluence page,
and a git-committed ADR. Without same-as resolution, the graph accumulates
duplicate decision nodes with no cross-reference.

This layer builds on the existing `find_document_similarity_matches` and
`DocumentReviewerAction` semantics from `src/ingest.rs`.

### 3.2 Cross-Source Same-As Resolution

**Where it runs**: after each connector version-walk import completes, a
same-as pass runs over the newly emitted decision nodes. This is a
**post-import advisory pass**, never inline in the write path.

```rust
pub fn find_connector_same_as_candidates<L: EventLedger>(
    ledger: &L,
    new_decision_ids: &[String],
    config: &SameAsConfig,
) -> Result<Vec<SameAsCandidate>>;

pub struct SameAsCandidate {
    pub left_id: String,
    pub right_id: String,
    pub score: u32,
    pub basis: SameAsBasis,
    pub review_required: bool,
    pub reviewer_action: SameAsReviewerAction,
}

pub struct SameAsBasis {
    pub algorithm: &'static str,
    pub title_token_overlap: u32,
    pub rationale_token_overlap: u32,
    pub topic_key_overlap: u32,
    pub connector_kind_match: bool,
    pub matched_fields: Vec<&'static str>,
}

pub enum SameAsReviewerAction {
    ReviewFuzzySameAs,
    ReviewAmbiguousSameAs,
}
```

**Matching algorithm**: generalized from `find_document_similarity_matches` to:
- Accept decisions from any `EventSource` (not just `EventSource::Document`).
- Require `score >= SAME_AS_MIN_SCORE` (tunable; default 70).
- De-prioritize exact same-connector matches (already handled by per-connector
  idempotency; same-as is for cross-connector dedup).
- Never fire on decisions that already have a `SAME_AS` edge between them.

**Precision bias**: false negatives (missed matches) are preferable to false
positives (incorrect merges). Default threshold is conservative: 70% overlap
on at least two of {title, rationale, topic_keys}. Do not lower this without
alex approval.

### 3.3 Same-As Links (Graph Representation)

Same-as links are edges, not merges. Decision nodes are never deleted or
replaced.

```
Decision A  --[SAME_AS { score, basis, confirmed_at, reviewer_id }]-->  Decision B
```

`SAME_AS` edges are **reversible**: a `relation.removed` event can retract one.
A retraction does not delete the evidence; it adds a `RETRACTED_AT` timestamp
on the edge.

Confirmed same-as pairs are stored as `relation.added` events with
`relation_type = "SAME_AS"`. Retracted pairs are stored as `relation.removed`
events. Both are auditable in the ledger.

### 3.4 Human Review Flow

Same-as candidates that exceed the threshold but require human confirmation
(score 70–85) use the `reviewer_action` pattern from
`DocumentReviewerAction::ReviewFuzzyDuplicateCandidate`.

```bash
# Inspect candidates
hivemind import connector same-as-candidates --since-run <import_run_id> --json

# Confirm one
hivemind import connector confirm-same-as \
  --left <decision_id_a> \
  --right <decision_id_b> \
  --actor human:alice

# Retract a previous same-as
hivemind import connector retract-same-as \
  --left <decision_id_a> \
  --right <decision_id_b> \
  --actor human:alice
```

SPA flow (non-technical users): same-as candidates surface in a review panel
with side-by-side decision text and a Confirm/Skip/Retract control. UX is part
of v1 scope for CLI; SPA review panel is post-v1 but the data contract is
defined here so it doesn't need rearchitecting.

### 3.5 Re-Import Awareness

If a connector document is re-imported (new versions available), the same-as
pass only fires for newly emitted decision nodes. Existing confirmed `SAME_AS`
edges are not re-evaluated (they were human-confirmed). Retracted `SAME_AS`
edges are not re-proposed (retraction is durable).

---

## 4. Invocation Surfaces

### 4.1 CLI

```bash
# Auth (one-time per connector)
hivemind connector auth gdocs           # opens browser for Google OAuth
hivemind connector auth confluence \
  --base-url https://myorg.atlassian.net \
  --token-env HIVEMIND_CONFLUENCE_TOKEN

# Import a document (walks all versions by default)
hivemind --actor human:alice \
  import connector \
  --url "https://docs.google.com/document/d/<id>" \
  --max-versions 20 \
  --json

# Import a git-tracked file
hivemind --actor human:alice \
  import connector \
  --url "./docs/ADR-001.md" \
  --max-versions 50 \
  --json

# Review same-as candidates after import
hivemind import connector same-as-candidates \
  --since-run <import_run_id> \
  --json
```

Output (JSON, `--json`): same shape as `hivemind import documents` —
`import_run_id`, `blocks_imported`, `blocks_conflicted`, per-statement results
with `reviewer_action`, `similarity_matches`, and `event_ids`.

### 4.2 Hosted API (Server-Side Import)

Non-technical users paste a URL into the SPA. The SPA calls:

```http
POST /v1/import/connector
Authorization: Bearer <session_token>
Content-Type: application/json

{
  "url": "https://docs.google.com/document/d/<id>",
  "actor_id": "human:alice",
  "max_versions": 20
}
```

The server resolves the connector from the URL, uses the caller's stored OAuth
grant for that connector, runs the same `Connector` + version-walk pipeline,
and returns the same JSON shape.

**Key invariant**: the hosted API path uses the identical `Connector` trait
implementation and pipeline code as the CLI. There is no separate "server
import" codepath. The difference is:
- CLI: connector credentials from local env / local token store.
- API: connector credentials from the auth sidecar's per-user grant store.

The connector pipeline is `async` throughout to support server-side concurrency
without blocking. CLI wraps the async runtime with `tokio::main`.

### 4.3 Connector Resolution (Multi-Connector Dispatch)

The pipeline holds a `Vec<Box<dyn Connector>>` (a registry). On import, each
connector's `resolve()` is called in order; the first `Some(source_id)` wins.
Unknown URLs return a clear error: "no connector matched this URL; supported:
gdocs, confluence, git-file".

---

## 5. Relationship to Existing Lanes

HiveMind currently has two ingestion lanes:

| Lane | Entry point | What it creates |
|------|-------------|-----------------|
| **Decision-block import** | `hivemind import documents` (src/ingest.rs) | Explicit `Decision` blocks → canonical events; deterministic; no LLM |
| **Prepare + Haiku classify** | `hivemind import prepare-documents` → `ingest` | PDF/OCR text → Haiku extractor → typed captures |

The connector pipeline is a **third lane**, not a replacement:

| Connector pipeline | `hivemind import connector` (new) | Source version history → statement-level diff → `decision.proposed` + `decision.superseded` chains |

### 5.1 Convergence or Separation?

**Proposal: keep lanes separate at the CLI boundary; share the event-emission
plumbing.**

Rationale:
- Decision-block import is the highest-fidelity lane (explicit human-authored
  blocks). Connector import is lower fidelity (statement segmentation from
  prose). Mixing them would obscure provenance.
- Both lanes write to the same ledger using the same canonical events and
  idempotency patterns, so the storage layer is already shared.
- The connector pipeline can reuse `import_document_decision_block` logic for
  conflict detection and `find_document_similarity_matches` for same-as scoring,
  but it does not call those functions directly from the hot path — it calls
  the shared inner logic once refactored into a `ingest::shared` module.

### 5.2 What Lands in V1 vs Deferred

**V1:**
- `Connector` trait + `ConnectorKind` enum.
- `GitFileConnector` (no auth, proves abstraction, dogfoods gas town docs).
- `GoogleDocsConnector` (Drive revisions API, OAuth via auth sidecar).
- Version-walk pipeline (list → fetch → segment → diff → emit chain).
- Statement segmentation (deterministic, no LLM).
- Same-as resolution pass (post-import, advisory, human-review gate).
- CLI: `hivemind import connector` + `hivemind connector auth`.
- API: `POST /v1/import/connector` (server-side, uses caller's OAuth grant).
- Same-as review commands: `same-as-candidates`, `confirm-same-as`, `retract-same-as`.

**Deferred:**
- `ConfluenceConnector` — auth adds complexity; defer to v1.1 once Google is
  working. (Open question for alex: is Confluence v1 or v1.1?)
- Standing watcher / continuous-poll daemon.
- SPA same-as review panel (data contract defined here; UX built later).
- LLM-assisted statement segmentation (layer 3; write path stays deterministic).
- Connector registry UI (add/remove connectors from SPA).
- Remote git (requires SSH/HTTPS credential management).

---

## 6. Narrowness Guardrail

From `CLAUDE.md` §2: nodes only earn a place by playing a role in decision
rationale. Connector import must not become a general-purpose knowledge graph
loader.

**Guard**:
- The statement segmentation step produces candidates. Before emitting a
  `decision.proposed`, the pipeline checks that the statement contains at
  least one of: a verb of intent ("decided", "will", "chose", "approved",
  "rejected", "adopted", "deferred"), or an explicit decision keyword from
  a configurable allowlist.
- Statements that fail this check are recorded as `source_ref`-linked evidence
  nodes (not decision nodes), available for later attachment to decisions.
- The allowlist is tunable per tenant and per import invocation; the default
  is conservative.
- Layer 3 may later suggest reclassification of evidence nodes as decisions
  (never the write path).

This keeps the graph a decision graph, not a document-content dump.

---

## 7. Open Questions for Alex

These are unresolved and require alex's call before implementation locks.

1. **Connector order**: should Google Docs or git-file ship first? Git-file
   requires no auth and dogfoods immediately; Google Docs is higher-value for
   non-technical users but adds auth complexity. Recommendation: git-file first
   (proves the abstraction cheaply), Google Docs second.

2. **Confluence in v1 or v1.1?** Confluence connector adds a second OAuth
   grant flow with less tooling. Recommendation: defer to v1.1 unless there
   is a specific user waiting on it.

3. **Default version-walk depth**: unlimited (walk all history) or a capped
   default (e.g., 20 versions)? Unlimited is more complete; capped avoids
   slow first imports on heavily-revised documents. Recommendation: cap at 50
   by default, `--max-versions 0` for unlimited.

4. **Confluence auth approach for v1**: Atlassian Cloud OAuth 2.0 (3LO,
   requires app registration) vs. API token via env var (simpler, but not SPA-
   compatible). Recommendation: API token for CLI v1, OAuth for SPA.

5. **Granularity choices**: should statement-level diffing emit one
   `decision.proposed` per statement, or group related statements under one
   decision node? One-per-statement gives maximum addressability; grouping
   reduces noise. Recommendation: one-per-statement for v1 (cleaner
   supersession chains); grouping as a layer-3 suggestion later.

6. **v1 API surface cut**: should the `POST /v1/import/connector` endpoint
   land in v1 or is CLI-only sufficient to validate the pipeline? Recommendation:
   CLI first, API endpoint in v1.1 once the pipeline is validated.

7. **Same-as threshold**: is 70% token overlap the right default for the
   precision-biased same-as gate, or should it be higher (e.g., 80%)?

---

## 8. Layers Compliance Check

| Concern | Layer | Verdict |
|---------|-------|---------|
| Connector.list_versions / fetch_version | Transport (zero-layer plumbing) | ✓ No ledger access |
| Statement segmentation | Layer 1 helper | ✓ Pure function, no LLM, no ledger |
| Event emission (decision.proposed, decision.superseded) | Layer 1 write | ✓ Appends canonical events |
| Same-as scoring (token overlap) | Layer 1 helper | ✓ Deterministic, no LLM |
| Same-as candidate surfacing | Layer 2 query | ✓ Read-only |
| LLM-assisted segmentation, smarter ranking | Layer 3 | ✓ Deferred, not in pipeline |
| Auth token refresh, OAuth grant storage | Auth sidecar | ✓ Not in core layers |

No layer boundary is crossed. Query layer does not write. Write path does not
call an LLM.

---

## 9. Provenance Contract

Every event emitted by the connector pipeline carries:

```rust
pub struct ConnectorSourceRef {
    pub connector_kind: ConnectorKind,
    pub doc_id: String,
    pub version_id: String,
    pub content_hash: String,          // SHA-256 of raw version bytes
    pub source_url: Option<String>,    // canonical URL for citation
    pub statement_ordinal: u64,        // 1-based index within version
    pub statement_span: (usize, usize), // byte range in normalized text
    pub import_run_id: String,
    pub importer_actor_id: String,
    pub original_actor_id: Option<String>, // from VersionMeta.author_actor
}
```

`occurred_at` = `VersionMeta.occurred_at` (source timestamp).
`recorded_at` = ledger append time.
`actor_id` = `original_actor_id` when present; otherwise `importer_actor_id`
marked provisional.

An imported event with no original author is marked `provisional: true` in
`source_ref` metadata, matching the existing pattern from
`TEXT_IMPORT_AND_DIFF_SEMANTICS.md §Provenance`.

---

## 10. Summary and Next Steps

This design delivers a clean connector abstraction (v1: Google Docs + git-file)
with version-walk, statement-level SUPERSEDES chains, bitemporal stamps, and a
precision-biased same-as layer with human-review gates.

**Immediate next steps (after alex sign-off):**
1. Alex reviews §7 open questions and unblocks the connector order and v1 cut.
2. Implementation bead for `Connector` trait + `GitFileConnector` + version-walk
   pipeline (no auth required — fastest validation path).
3. Implementation bead for `GoogleDocsConnector` + auth sidecar connector token
   storage.
4. Implementation bead for same-as layer + review commands.
5. Implementation bead for `POST /v1/import/connector` API endpoint (if v1).

Nothing in steps 2–5 begins before alex reviews §7.
