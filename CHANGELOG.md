# Changelog

All notable changes to HiveMind are documented here.
This project adheres to [Semantic Versioning](https://semver.org/).

## Unreleased

### Added

- **`GET /v1/graph` says who decided what and whether it held up.** Every `decisions[]` entry
  carries its derived `status` (`proposed`, `accepted`, `rejected`, `contested`, `superseded`)
  and `deciders` (the actors who accepted it, each with `kind` `human` or `agent`; empty until
  someone accepts), and every Option node carries a `title` (its label, else its id).
  Additive: no existing key changes. Documented in `docs/GRAPH_CONTRACT.md`. (hivemind-pkw0)
- **`GET /v1/whoami` says who a token is.** It takes the same bearer credential as every other
  `/v1` route (user token, agent token, WorkOS sign-in, the shared `HIVEMIND_API_KEY`, a
  Postgres tenant token) and answers `{"actor_id", "tenant_id"}` for it, or `401` with the usual
  error body for a missing, empty, unknown or revoked token. The shared key and a tenant token
  answer `service:api`, so a client can warn that writes made with them carry no person's name.
  Opens no ledger and writes no event; there is no MCP tool for it. Documented in
  `docs/SELF_HOSTING.md`. (hivemind-jro0)

### Fixed

- **"Why did we pick / choose / go with X?" answers in one step.** The verbs people ask about a
  decision with (pick, choose, decide, "go with", "settle on", "opt for", with their past and
  `-s` forms) and the adverbs they put in a why-question (still, again, ever, even, really,
  actually, now, anymore, currently) are now question words. Asking `why did we pick shadcn for
  the design system` or `why is the courtroom demo still on the site` resolves to the decision
  and its rationale instead of listing it as a close candidate that lacks "pick" or "still".
  The list is fixed and applies to the question only, so a decision titled "Pick the cheapest
  vendor" still matches on "pick", and "go" alone stays a search term. `recall` drops the same
  words. (hivemind-36vt)
- **`/hivemind-capture:query-decisions "some words"` no longer fails on the quoted form its own
  argument-hint documents.** The command wrapped what you typed in a second pair of quotes, so
  `"current-project setting"` reached the CLI as two words (`error: unexpected argument
  'setting' found`) and any flag typed after the query was swallowed into it. It now passes
  your arguments through as typed and joins the leading free-text words into one query, so
  the quoted and unquoted forms both work. (hivemind-f4ng)
- **`recall`, `why`, `verify`, `disagree` and `supersede` in the hivemind-context plugin print
  their usage when called with no arguments**, instead of dying on macOS's bash 3.2 with an
  unbound-variable error. (hivemind-f4ng)
- **`--graph-backend kuzu` works again.** It is only there in a binary built from source with
  `--features graph-kuzu`, and building its graph from a ledger stopped at the first event: the
  Kuzu graph had no column for an actor's kind and no table for the `SAME_AS`, `PARTICIPATED_BY`
  and `INITIATED_BY` links, nor a column for an evidence capture's topic keys or a decision's
  score, and it could not store a blocker resolved with a reason but no event id. A
  `graph.kuzu` left by an earlier build needs nothing from you: every run rebuilds it from the
  ledger. (hivemind-cbab)

## v0.7.0 — 2026-09-25 — M6: Fluent verbs and grounded capture

You can now consult HiveMind without holding a decision id. Describe a decision — or just
ask "why did we…?" or "did that hold up?" — and `why`, `verify`, `disagree` and `supersede`
find it, refusing to guess when the description is ambiguous; `recall` and `situational`
answer from a question or from the files you are touching. `why` now answers *why*: rationale,
the options chosen and rejected, who decided and whether it still holds, not a bare list of
ids. And every captured decision must now say what it rests on; when a premise is later
superseded, the decisions that followed from it say so.

This release has breaking changes, several of which reject input v0.6.0 accepted. Read
**Breaking changes** before upgrading, and upgrade every reader of a shared ledger before any
v0.7.0 client writes to it.

### Breaking changes

- **Capture and supersede require grounding.** Every capture answers "what does this decision
  rest on?" and is refused — nothing written — when the answer is empty. There are four ways to
  answer: a decision already made, something observed (and where), something assumed, or, when
  there is nothing yet, a declared **bet** ("nothing yet" is a bet, optionally with what would
  change your mind and a check-by date). An existing evidence or hypothesis id also counts. The
  decider's own words are not a grounding; they go in `--quote` / `--question`.
  (hivemind-gwhr.1, hivemind-gwhr.2)
  - CLI: `emit decision.capture` and `supersede` take `--rests-on-decision
    <description|#N|decision-id>`, `--rests-on-evidence <observation>` with `--evidence-source`,
    `--rests-on-assumption`, `--bet [statement]` with `--would-change-if` / `--check-by`, and
    `--confidence low|medium|high`. A capture that names none exits **2** with the four ways to
    answer. A premise description that is ambiguous or matches nothing also exits 2 and writes
    nothing; ambiguous ones list numbered candidates to re-run with `--rests-on-decision '#N'`.
  - MCP (stdio and HTTP): `capture_decision` and `supersede_decision` require a `grounding`
    array of typed items (`decision`, `evidence`, `assumption`, `bet`). An empty one returns a
    tool result with `isError: true` and the four ways to answer. `hypothesis_ids` /
    `evidence_ids` remain as deprecated aliases and count as grounding. An ambiguous or unmatched
    premise is a *successful* `{outcome: "ambiguous" | "not_found", field: "grounding[i]"}`
    result with no event appended.
  - HTTP: `POST /v1/decisions` and `POST /v1/decisions/{id}/supersessions` take the same
    `grounding` array; an empty one is **HTTP 400** (`validation_error`) with the four ways to
    answer.
  - Not asked: raw `emit decision.proposed`, classifier ingest, document import, Slack import and
    the interactive `review` supersede.
  - **The capture plugins do not ask the question yet.** The hivemind-capture skill, its
    commands and the Codex bundle were not updated in this release (hivemind-gwhr.5): until that
    plugin update ships, an agent capturing through them meets the refusal text — which does
    name the four ways to answer — instead of being prompted for grounding up front.
- **Capture rejects input v0.6.0 accepted.** Rejected outright, never truncated or repaired:
  - a rationale under 20 characters or 4 words, or one that leans on a bare reference into a
    list that isn't in it (`1a`, `2. a`) — pair `--quote` with `--question` instead
    (hivemind-763i). `--quote` and `--question` must be given together (hivemind-zdsh.13);
  - a title over 120 characters, of more than one sentence, or that is a numbered list
    (hivemind-zdsh.12);
  - an option label over 80 characters, containing "chosen", reading as a "rejected options"
    bucket, or bundling several numbered answers. Option ids are now opaque
    (`option-<uuid>`) rather than a slug of the label; existing slug ids display a readable
    label (hivemind-zdsh.10);
  - a bare UUID as an actor id — use `human:<name>` or `agent:<tool>:<name>`. Agent ids are now
    derived from a durable identity (`GC_AGENT` / `GC_ALIAS`) before falling back to session
    environment variables, so one agent keeps one actor id across restarts; the raw session id
    moves to `source_ref` (hivemind-zdsh.9).

  Ledger replay of events captured earlier is unaffected.
- **A chosen option now self-accepts.** A capture that names a chosen option is `accepted`
  immediately — by `--decided-by` when given, otherwise by the recording actor — instead of
  staying `proposed` forever. Pass `--still-proposed` (`still_proposed` over MCP and HTTP) to keep
  a genuine open recommendation at `proposed`. `supersede` and document import are unaffected.
  (hivemind-zdsh.8)
- **A ledger holding v0.7.0 writes cannot be read by v0.6.0.** v0.6.0 refuses the new fields —
  `unknown field 'option_descriptions'` on `decision.proposed`, `unknown field 'kind'` on
  `hypothesis.recorded` — so a single v0.7.0 capture is enough. Upgrade every server, CLI and
  plugin that reads a ledger *before* any v0.7.0 client writes to it. Ledgers written by v0.6.0
  replay unchanged. New event
  types: `project.registered`, `project.linked`, `project.unlinked`, `project.anchored`,
  `project.unanchored` and `decision.moved`; new optional fields on `decision.proposed`,
  `decision.accepted`, `hypothesis.recorded` and `ingest.batch_classified`; a new relation kind,
  `FOLLOWS_FROM`. Postgres projections add the new columns automatically.
- **`hivemind serve` binds loopback by default.** v0.6.0 bound `0.0.0.0` and, with no
  `HIVEMIND_API_KEY`, started with no authentication — an open server on every interface. It now
  binds `127.0.0.1` (`--bind` / `HIVEMIND_BIND`), and in that development mode (no
  `HIVEMIND_API_KEY`, no `HIVEMIND_DATABASE_URL`) it refuses to start on a non-loopback bind
  unless `--allow-unauthenticated-remote` is passed. An empty `HIVEMIND_API_KEY` or
  `HIVEMIND_ADMIN_KEY` now counts as unset (see Security under Fixed). In Docker the image binds
  `0.0.0.0` inside the container and `docker-compose.yml` publishes on `127.0.0.1`
  (`HIVEMIND_PUBLISH_ADDR` to expose it deliberately); SQLite mode in compose refuses to start
  without `HIVEMIND_API_KEY`. The image no longer bundles the website. (hivemind-4dur)
- **An unregistered tenant is an error.** An unknown `--tenant` / `X-HiveMind-Tenant` now fails
  reads and writes on both backends (`unknown tenant 'acme': run hivemind tenant create acme`)
  instead of silently opening an empty scope. `local` is always registered. On SQLite,
  `hivemind tenant create <id>` registers one — a v0.6.0 ledger that used a non-default tenant
  needs that once before use. On Postgres, tenants are provisioned by the server's provisioning
  route. (hivemind-rkbf.1)
- **Every arrow the server shows points newer → older.** `GET /v1/graph` edges, the
  neighborhood edges (`hivemind query why`, MCP `get_decision_neighborhood`, HTTP
  `/v1/decisions/why`), the CLI text summary and the DOT exports now draw each edge from the
  node recorded later to the node recorded earlier, for all 28 relation kinds. `from`/`to`
  are the arrow's ends, `relation` still names the meaning, `label` reads the relation along
  the arrow (`based on`, `informs`, `answers`) and `reversed` marks an arrow that runs against
  the relation's stored direction. Storage and queries are unchanged, so nothing needs
  migrating: graphs rebuild from the ledger. The rule and the per-kind table are in
  `docs/GRAPH_CONTRACT.md`. (hivemind-ku1x)
- **Follow-up events no longer overwrite `event_origin`.** `decision.scored`,
  `blocker.resolved`, `notification.acknowledged` and `project.anchored` leave the
  `event_origin` of the node they annotate at the offset of the event that created it. A
  persistent Postgres projection needs the usual rebuild to pick that up. (hivemind-ku1x)
- **Other wire changes.**
  - `GET /v1/decisions/{id}` returns 404 `not_found` for a missing decision (or another
    tenant's) instead of 200 with `data: null`.
  - `hivemind --version` and the MCP `serverInfo.version` read `<semver>+<commit>` (the
    commit's first 12 characters), not bare semver; `GET /v1/version` returns the same.
  - Capture and supersede replies gain `rests_on` and `premise_stale`, and on the CLI's JSON
    output and MCP `project` and `project_source`. Text mode still prints the bare decision id on
    stdout; the project line goes to stderr and each stale premise adds a `premise_stale` line.
  - `hivemind digest` text shows option titles rather than option ids, gains a `rests on:` line
    per decision and no longer ends with a `Cited:` line (the JSON keeps `cited_decision_ids`).
- **The website and docs moved; the hosted open beta is gone.** They now live in
  `alexknips/hivemind-site`; `website/` left this repo and the old GitHub Pages URL only
  redirects. The managed MCP server is down and nothing points at it: self-hosting — the release
  tarball or the ghcr image — is the only install path. (hivemind-se2a, hivemind-v5lv)

### Added

#### Fluent verbs: describe a decision, don't quote its id
- **`disagree`, `supersede`, `query chain`, `query why`, `query verify` and `query compact-view`
  take a description.** A positional description resolves to one decision; `--pick N`, `--topic`
  and a bare `#N` (from the previous ambiguous result) narrow it. A description that does not
  resolve to exactly one decision writes nothing and lists numbered candidates; `--id` /
  `--decision` / `--old` remain. The `#N` state is a local file scoped by `$HIVEMIND_SESSION`,
  never part of the ledger. (hivemind-tenv.1)
- **`why` answers why.** The neighborhood root carries the decision's brief — title, rationale,
  chosen and rejected options, who decided, whether it still holds — and every non-actor node
  carries a label. `verify` ("did it hold up?", `query get_decision_outcome`) leads with the same
  brief and shows the recorder and the decider as separate lines, saying "not yet decided" when
  nobody accepted. Natural questions resolve: question words are dropped, negations are kept so
  "do not adopt X" never resolves to the decision that adopted X, and when nothing matches every
  word, close candidates are listed with their `missing_terms` instead of `not_found`.
  (hivemind-5gwg)
- **`recall` takes its documented question form.** `recall "what did we decide about projects"
  --topic projects` drops the question words, reports them as `ignored_words`, and lets `--topic`
  decide when nothing else is left. (hivemind-5gwg)
- **`query situational` — "what should I know before I touch this?"** Decisions bearing on the
  working situation (touched paths, a diff, the current branch, the cwd) with no question needed;
  it defaults to the current git diff, and can be asked from one project (see Projects).
  (hivemind-tenv.2)
- **Over MCP and HTTP too.** The same verbs are HTTP routes (`GET /v1/decisions/recall`,
  `/why`, `/verify`, `/situational`) and MCP tools. New MCP tools on both transports:
  `get_decision_neighborhood`, `get_situational_decisions`, `scan_misfiled_decisions`,
  `classify_queue_list` and `classify_queue_submit`. `disagree_decision`,
  `get_decision_outcome`, `get_supersession_chain` and `hivemind_compact_view` accept
  `description` (+ `topic`) in place of `decision_id`, and `supersede_decision` in place of
  `old_decision_id`. (hivemind-ot72)
- **`hivemind-context` plugin.** A CLI-only Claude Code and Codex plugin — `situational`,
  `recall`, `why`, `verify`, `disagree`, `supersede` — that never asks the agent for a decision
  id. (hivemind-tenv.3)

#### What a decision rests on
- **Answers show what a decision rests on.** Every decision view gives each premise with its
  state — a decision that holds, was superseded, rejected or is contested; evidence with where it
  was seen; an assumption that is open, supported or refuted; a bet that is open, held or failed
  — and whether it was named at capture or attributed later, plus the expressed confidence and
  how many decisions rest on it. A superseded or rejected premise makes the decision **stale**
  in `verify`, `compact-view`, `situational`, `digest`, recent-activity history and the scorer;
  a contested premise is shown, not stale; an overdue bet is reported `unchecked` and does not
  flip "still holds". A decision captured before this release reads "nothing declared (never
  asked)". (hivemind-gwhr.3)
- **`hypothesis.recorded` gains `kind` (`assumption` | `bet`), `check_by` and `would_change_if`;
  a decision can `FOLLOWS_FROM` another decision** (`emit relation.added --kind follows-from`).
  (hivemind-gwhr.1)

#### Projects
- **Every decision carries its project and how that was determined.** `--project` /
  `--project-source` on `emit decision.capture`, `emit decision.proposed` and `supersede` (MCP:
  `project`, `project_source`). HiveMind checks the handle and never infers it: an unregistered
  handle is refused, and with none the decision is saved to the recorder's personal project and
  the reply says so (`personal_fallback`). `supersede` inherits the old decision's project.
  (hivemind-s15q.1–.4)
- **Every answer names its project.** Each decision that `get_decision`, `search`, `recall`,
  `situational`, `why`, `verify`, `compact-view`, `recent` and `digest` return, and the
  decision log (`hivemind export`), carries `project` (the address) and `project_label` (what a
  person calls it): the display name it was registered with, else the handle; for a personal
  project, "alex's personal project" or "claude agents' personal project". A moved decision
  names the project it moved to. A decision that a request or blocker named before any proposal recorded
  it has no project: `project` is `null` and the label reads "no project recorded", so it is
  visible rather than blank or guessed. The text output gains a trailing `project=<label>` field
  on tab-separated rows and a `project: <label>` line on paragraph-shaped ones (`verify`,
  `compact-view`, `digest`, the decision log, the TUI and Slack answers). A decision captured
  from a classified batch is filed under the recording actor's personal project.
  (hivemind-s15q.5)
- **`hivemind move` — move a decision to another project by describing it.** `hivemind move
  "<description>" --to <handle> [--pick N] [--topic T] [--reason R]`, or `--decision <id>`,
  resolves the decision the way `disagree` and `supersede` do: a description that matches more
  than one decision lists numbered candidates and writes nothing, and one that matches nothing
  is a successful `not_found` answer. Where the decision is now is read from the ledger, never
  typed; `--to` must be a registered project or your own personal project, and a decision
  already there is refused. Each move is recorded (who, when, from, to, why), keeps the
  decision's original capture provenance, and shows in `get_recent_activity` and
  `get_decisions_changed_since` as a `project_moved` change carrying `project_move`
  (`from`, `to`, `reason`); it is reversed by moving the decision back. Over MCP it is
  `move_decision` on both transports, taking `decision_id` or `description` (+ `topic`), `to` and
  `reason`, and replying `{decision_id, event_id, from, to, reason?}` like `move --json`.
  (hivemind-s15q.11)
- **"What should I know" asks from one project.** `hivemind query situational --project <handle>`
  (MCP `get_situational_decisions`, argument `project`) takes a registered handle or a personal
  address such as `personal:human:alex`. Matches come from that project first, then from the
  project it is `part_of` (an inherited constraint, labelled "from Platform; Billing is part of
  it"), then from the projects it `depends_on` (one hop, "from Auth; Billing depends on it");
  each group keeps the usual score order and paging. Every match carries a `scope` (`relation`,
  `label`), and the answer carries a `scope` note — on an empty answer too — naming the projects
  it looked in and how many more levels up the `part_of` chain and how many more linked projects
  were not followed, so a short answer never reads as a complete one. Decisions filed under any
  other project, and decisions with no project recorded, are not in a scoped answer. Superseded
  and refuted labels apply across the hop unchanged. An unregistered handle is refused with the
  `hivemind project register` hint, never answered as an empty result. `--summary` adds a `scope`
  line and a `scope=<label>` cell on each row. Links are read from the ledger, so a retracted
  link is not followed. Without `--project` the answer is unchanged, and `GET
  /v1/decisions/situational` does not take `project`. (hivemind-s15q.6)
- **`hivemind project`** — `register`, `link` / `unlink` (`part_of`, `depends_on`), `anchor`
  (folder, rig, Jira, Linear, GitHub, channel), `list`, `show`, `use` (a per-machine current
  project) and `decisions <handle>` (paged, with `truncated` and a cursor). (hivemind-s15q.2,
  .9, .13)
- **`hivemind export --format markdown --out <dir>`** writes the decision log as Markdown
  grouped per project — an `INDEX.md`, then one directory per project; `--project`, `--since`
  and `--topic` filter. The export owns its tree and prunes stale files. (hivemind-xw61)
- **`scan_misfiled_decisions`** (`query scan_misfiled_decisions`, MCP) reports decisions carrying
  a topic key you name as foreign. It only reports; to relocate one, use `hivemind move`.
  (hivemind-zdsh.14)

#### Attribution
- **`--decided-by`** records the human who decided when an agent captures: the decision is
  accepted by them and `verify` shows both. **`--delegated-by human:<name>`** marks an agent
  deciding within a human's delegation, so it reads differently from an agent deciding alone;
  `digest`, `verify` and the Markdown decision-log export show it and failure-mode attribution
  gains a `delegation` dimension. (hivemind-zdsh.3, hivemind-zdsh.6, hivemind-o7p2)
- **`POST /v1/agent-tokens`** (admin) mints a token bound to `agent:<tool>:<name>`, so a shared
  per-role token attributes its writes to an agent rather than a human. (hivemind-zdsh.19)

#### Backends and operation
- **Backend selection for the CLI and stdio MCP:** `--database-url` / `HIVEMIND_DATABASE_URL`
  points them at the shared Postgres backend, as `hivemind serve` already could. It needs a build
  with the `shared-backend-postgres` feature — the ghcr image has it, the release tarballs do
  not (they refuse with a clear error). `docker-compose.local-agents.yml` publishes Postgres on
  `127.0.0.1` for agents outside the compose network. (hivemind-ot72.2, hivemind-ot72.15)
- **Slack front door.** `POST /v1/slack/events`, `POST /v1/slack/commands` and
  `GET /v1/slack/oauth/callback`, with request-signature verification; a workspace maps to one
  tenant. Limits: SQLite backend only, `reaction_added` is verified and acknowledged but does not
  capture, and there is no modal. (hivemind-s5rs)
- **Classification over HTTP and MCP.** `classify-queue list` / `submit` use `GET /v1/classify-queue`
  and `POST /v1/classify-queue/submit` when `HIVEMIND_API_URL` is set. One submission can cover
  several batches of a session (`--batch-id a,b`, `--session-id`), and a shared daily cap
  (`HIVEMIND_CLASSIFY_DAILY_CAP`, default 150) leaves the rest pending for the next UTC day.
  (hivemind-zdsh.18)
- **`emit decision.scored`** submits a quality score computed at the edge, so a plugin can score
  without `ANTHROPIC_API_KEY` on the server. (hivemind-wi3u)
- **A build stamp.** `hivemind --version`, `GET /v1/version` and the MCP handshake report the
  commit. `make install` now installs to `~/.local/bin`, where `scripts/install.sh` puts
  releases; `scripts/install-local.sh`, `scripts/cell-update.sh` and `scripts/check-freshness.sh`
  build, roll and check a self-hosted cell. (hivemind-zdsh.7)

### Fixed

- **Security: an empty admin key no longer opens the admin routes.** A v0.6.0 server started
  with `HIVEMIND_ADMIN_KEY=""` — which the shipped `docker-compose.yml` passes when the variable
  is unset — let a request with no token, or an empty bearer, create users and mint tokens
  through `POST /v1/users`. An empty key now counts as unset and those routes refuse. If such a
  server was reachable by anyone you do not trust, review the users and tokens it holds.
  (hivemind-4dur)
- **"Did it hold up?" works.** `get_decision_outcome` and `get_decision_context` failed with
  `graph projection error: unsupported query` on the default in-memory graph — the backend the
  MCP server always uses — and on Postgres. Both are fixed. (hivemind-tenv.1, hivemind-kj0i,
  hivemind-ookw)
- **Attribution is honest.** `supersede_decision` and `disagree_decision` over MCP no longer
  label an agent `agent:codex:…` whatever its tool; the CLI's `disagree` and `supersede` record
  `source=agent` for an agent actor instead of `human`; `verify` no longer prints the recorder as
  the decider; the classifier no longer folds a per-run session id into an actor id.
  (hivemind-zdsh.9, hivemind-xm93, hivemind-zdsh.19)
- **Passive capture no longer drops the session.** The capture hook reads `session_id` and
  `transcript_path` from the hook's stdin, where Claude Code provides them, instead of returning
  silently when `CLAUDE_SESSION_ID` is empty, and resolves transcript paths containing dots and
  underscores. It advances its cursor only after a successful POST, ships every turn of a span
  oldest-first, and ships a typed prompt verbatim rather than character by character.
  (hivemind-zdsh.17, hivemind-8h7m)
- **Plugin scripts.** `capture.sh` no longer drops `--decided-by`'s value; the
  `hivemind-context` verbs join unquoted multi-word text; `query-decisions.sh` has honest
  defaults and quotes its arguments; the capture skills say to `recall` before capturing so a
  restarted session does not record a decision twice. (hivemind-zdsh.3, hivemind-tenv.3,
  hivemind-tenv.4, hivemind-zdsh.11)
- **Postgres.** Concurrent first starts against a fresh database no longer race on schema
  creation (advisory lock); `HIVEMIND_POSTGRES_POOL_SIZE` overrides the connection pool size.
  (hivemind-g9kv, hivemind-ot72.3)
- **Classifier.** A capture that names a sibling capture in the same batch by title now resolves
  to that node's real id; duplicate-title resolution warns and is counted.
  (hivemind-o4l6, hivemind-11nl)
- **Situational.** A decision with two `BASED_ON` edges to the same evidence no longer lists the
  match twice. (hivemind-nw9v)

### Known issues

- `verify` on a decision captured without grounding suggests `hivemind ground …`. That verb is
  not in this release; to attach a premise after the fact use
  `emit relation.added --kind follows-from --from <decision-id> --to <premise-decision-id>`.

---

## v0.6.0 — 2026-09-07 — M7: Decision-quality layer

HiveMind now derives whether decisions held up and scores them explainably — entirely
from graph structure, no LLM, no external calls required. An engineer and their agents
can see which decisions didn't hold up, why, and what conditions predict failure — without
any per-person ranking or blame. Scores always ship with their reasons and contributing
decision IDs. External consumers (factory loops, dashboards) pull the same signal via MCP.

### Added

#### Decision outcome signals (Mechanism foundation)
- **Decision outcome model.** Layer-2 query (`src/queries/outcome.rs`) derives
  four per-decision quality signals from existing graph edges — `superseded_fast`
  (replaced within one sprint), `premised_on_refuted` (rested on a hypothesis
  later contradicted by evidence), `contested_unresolved` (active disagreement with
  no resolution), and `thin_structure` (no options considered or evidence attached).
  No LLM, no external calls; works self-hosted.
  MCP tools: `get_decision_outcome`, `decision_quality_candidates`.
  (hivemind-he9a.1, 6085596)
- **Decision context features.** Layer-2 query (`src/queries/context.rs`) derives
  the independent-variable side of the causal pair: authorship shape (human/agent/joint),
  source, model, review depth, evidence and hypothesis counts, rationale richness proxies.
  MCP tools: `get_decision_context`, `decision_context_candidates`.
  (hivemind-he9a.2, a779f60)

#### In-house explainable scorer
- **Decision-quality scorer.** Layer-3 scorer (`src/queries/inhouse_scorer.rs`) combines
  outcome signals and context features into a `[0,1]` quality score plus a tier label,
  always with contributing reasons and decision IDs attached — never a bare number.
  Deterministic; no LLM; self-hosted and hosted. Scores decisions and interaction patterns,
  never individual people or agents.
  MCP tools: `score_decision`, `scan_decision_quality`.
  CLI: `hivemind query scan_decision_quality`.
  (hivemind-he9a.3, b006942)
- **Failure-mode attribution.** Layer-2 query (`src/queries/attribution.rs`) reports
  aggregate condition patterns (model, context sufficiency, review depth, evidence
  thinness, authorship shape) with effect sizes and honest confidence intervals.
  Output is aggregate-only — no per-person rankings, no individual identifiers.
  MCP tool: `analyze_failure_modes`.
  (hivemind-he9a.4, ba5178a)

#### MCP exposure — Mechanism A
- **Quality scores over MCP.** External consumers (factory loops, dashboards, other agents)
  pull decision scores, signals, context features, and failure-mode analysis via the existing
  MCP auth surface (hosted + self-hosted). Low-effort pull model; no new infrastructure.
  Adds hosted-mode routing in `src/api.rs` and CLI wiring.
  (hivemind-he9a.6, 14ae063)

#### Scheduled scan → Linear tickets — Mechanism B
- **Quality-scan CLI + Linear connector.** `hivemind quality-scan` schedules periodic scans
  of recent decisions and files Linear tickets for human review on decisions above a
  configurable badness threshold. Precision-biased (files only on strong signals);
  human-in-the-loop mandatory; opt-in (requires `HIVEMIND_LINEAR_API_KEY`). Reuses
  the in-house scorer — no duplicate scoring logic.
  (`src/linear.rs`, hivemind-he9a.7, be93351)

#### E2E test suite
- **SQLite smoke suite + CI job.** Compose-based e2e smoke test (SQLite backend) runs
  in CI on every push. (hivemind-fabq.1, 465277c)
- **Postgres smoke suite + CI job.** Compose-based e2e smoke test (Postgres backend)
  with tenant provisioning and bearer-token auth runs in CI. (hivemind-fabq.2, 2024c3e)

### Changed
- **`docs/DECISION_SCORING.md`** status updated to reflect shipped implementation.
- **`STRATEGY.md`** gains an 8th active front: Decision quality.
- **README install snippet.** `HIVEMIND_VERSION` example updated to `v0.6.0`.

---

## v0.5.0 — 2026-08-21 — Import unification: prose extraction and clean CLI

`hivemind import` now works end-to-end for real documents. The command
auto-detects whether input is raw prose or a pre-structured block list and
extracts decision candidates inline — no pre-processing step required.
Imported decisions land as **unreviewed** (status `proposed`) and are
surfaced by `hivemind review --unreviewed-only`, closing the review loop.
The experimental `suggest document-candidates` and `materialize-document-candidates`
sub-commands, superseded by this flow, are retired.

### Added
- **Prose-aware import.** `hivemind import` auto-detects prose vs block input
  and runs inline extraction when prose is supplied, eliminating the separate
  `suggest → materialize` step. (`src/import.rs`, 21a2368, f90992c)
- **Unreviewed landing.** All imported decisions land with status `proposed`
  (unreviewed), surfaced by `hivemind review --unreviewed-only`.
  (21a2368)

### Removed
- **`hivemind suggest document-candidates`** — superseded by auto-detection in
  `hivemind import`. (2268b1e)
- **`hivemind materialize-document-candidates`** — superseded by inline
  extraction in `hivemind import`. (2268b1e)

### Changed
- **README install snippet.** `HIVEMIND_VERSION` example updated to `v0.5.0`.
- **Docs.** Import/review flow docs updated to reflect the new single-command
  path; stale `suggest`/`materialize` references removed. (7f05a18)

### Improved
- **Hosted server hardening.** Per-request timeouts, concurrency limits, and
  JWKS refresh robustness. (hivemind-u6uu)
- **Graph caching.** Projected graph cached in `AppState` with incremental
  offset-keyed invalidation — reduces per-request rebuild latency.
  (94717be)
- **`premised_on` edge origin.** Phase 2: `PREMISED_ON` edges now originate
  from the chosen `Option` node rather than the `Decision`, matching the
  semantic intent. (hivemind-1ysb)
- **Website.** Added "Use cases" (Shipped vs Planned) and "Why HiveMind"
  narrative pages. (96f911a, cd6103e)

## v0.4.1 — 2026-07-21 — Self-host server image on ghcr

A packaging release that completes the self-hosting deployment path. No
functional changes to the `hivemind` binary since v0.4.0.

### Added
- **Self-host server image on ghcr.** Tagged releases now build and publish
  the multi-tenant server image to `ghcr.io/alexknips/hivemind` (tags
  `:latest`, `:0.4`, `:0.4.1`), built with the `shared-backend-postgres`
  feature. `docker-compose.yml` now pulls this prebuilt image, with the local
  `build:` retained as a fallback — so `docker compose up` no longer requires
  a ~5-minute local Rust build on first run. (hivemind-4qj7)

### Changed
- **README install snippet.** `HIVEMIND_VERSION` example updated to `v0.4.1`.

## v0.4.0 — 2026-07-20 — M4: Self-hosting GA, keyless capture, and Ingestion v1

Organizational self-hosting exits beta: per-user bearer tokens, a
compose-first deployment cell, WorkOS AuthKit OAuth for the hosted MCP
gateway, and self-host polish bring the self-hosted path to GA readiness.
Keyless capture (no `ANTHROPIC_API_KEY` on the server) ships via a
subscription-seat Worker A path. Ingestion v1 lands as **experimental**
connectors: a `Connector` trait, `GitFileConnector`, `GoogleDocsConnector`
(OAuth), and a same-as/dedup layer with same-as review commands. Layer-3
gains a 2-axis decision scorer and a semantic spectral decision map; the CLI
adds `recall` and `digest` subcommands; the MCP gateway adds HTTP Streamable
transport and a `recall_decisions` tool. Fidelity measurement matures via a
36-case gold corpus, A/B harness, and Tier 1.5 synthetic dispersion cases.

Prebuilt binaries ship for **linux-x86_64, linux-arm64, and macos-arm64**.
Intel macOS (`x86_64-apple-darwin`) is not currently published as a prebuilt
— the ONNX runtime that backs the spectral map ships no prebuilt for that
target — so Intel Mac users install via `cargo install --git`.

### Added

#### Self-hosting and auth
- **Per-user bearer tokens** for self-hosted cells. Each provisioned user
  receives an `hm_tk_<64-hex>` token validated against a per-cell token
  store. (`src/ledger/postgres/tenant_store.rs`, `src/ledger/sqlite/mod.rs`,
  hivemind-iioq, f57dd30)
- **Self-hosted cell compose wiring.** `docker-compose.yml` ships a
  production-ready cell configuration — env surface, volume mounts, and
  health-check; `docs/SELF_HOSTING.md` is the authoritative runbook.
  (hivemind-ppw3, 5780e7a)
- **WorkOS AuthKit OAuth.** The hosted MCP gateway accepts WorkOS-issued
  JWTs, enabling browser-based GitHub/Google login for MCP clients.
  (`src/api.rs`, hivemind-k6hv, 74d7ec0)
- **CORS layer**, opt-in. Browser SPA clients can call `/v1/` directly when
  origins are set via `HIVEMIND_CORS_ORIGINS`; off by default, so
  self-hosters are unaffected unless they enable it. (hivemind-2l8h, 936a3d7)
- **`hivemind digest` command.** Generates a weekly decision-digest summary
  for a given time window. (`src/cli/mod.rs`, `src/summarize.rs`,
  hivemind-k125, d798df7)
- **`hivemind migrate` CLI**, behind the `shared-backend-postgres` build
  feature (off by default). Migrates a local SQLite ledger to a remote
  Postgres-backed shared backend. (hivemind-uuq9.8, d4b7e7b)
- **SPA static serving + `GET /v1/graph`.** The server serves the SPA demo
  bundle and exposes `GET /v1/graph`, which returns the full decision graph
  as JSON (nodes + edges, no coordinates). The 2-D spectral *layout* is a
  separate endpoint — see the decision map below. (98fb8a7)

#### Keyless capture (no server-side ANTHROPIC_API_KEY)
- **Worker A — subscription-seat keyless path.** `hivemind classify-queue`
  CLI and capture plugin drain unclassified batches using the subscription
  seat's API key, not the server's. Enables zero-API-key self-hosted
  deployments. (hivemind-v8im, 731d367)
- **Haiku edge classifier subagent.** The capture path (`hivemind emit`
  plus the capture plugin/skill) can classify graph edges (actor, evidence,
  option relationships) at capture time via a local Haiku call rather than
  the server-side classifier. (hivemind-mfc7, 97f869d)
- **Keyless capture docs and onboarding.** `docs/KEYLESS_CAPTURE.md` —
  zero-to-first-decision walkthrough. The self-hosting funnel (README,
  `docs/SELF_HOSTING.md`, `docs/AGENT_DECISION_CAPTURE.md`) updated to
  surface the keyless path. (hivemind-kj84, 0268e8b)

#### Ingestion v1 connectors (experimental)
> These connectors are **experimental** — the `Connector` trait and
> ingestion API are subject to change before a stabilization milestone.

- **`Connector` trait + version-walk pipeline.** Common interface for
  pull-based ingestion; `GitFileConnector` walks git history and extracts
  decision candidates per version. (`src/connector/`, hivemind-2tcl.1,
  edd2af4)
- **`GoogleDocsConnector`.** Ingests Google Docs via Drive API OAuth2 flow
  with credentials and token caching. (hivemind-2tcl.2, ced6ed9)
- **Same-as / dedup layer.** Deduplicates connector output across ingestion
  runs; `hivemind import connector same-as-candidates` /
  `confirm-same-as` / `retract-same-as` for manual review of same-as
  candidates. (hivemind-2tcl.3, 7d7269a)

#### Layer-3 intelligence
- **2-axis decision scorer.** Layer-3 scorer annotates decisions (via
  `decision.scored` events) on two axes: **Quality** [0,1], a weighted
  composite over 7 dimensions, and **Importance** (Stakes × Irreversibility
  × Actionability). Swappable — query and write layers are unaffected. Note:
  scoring calls a model and requires a server-side `ANTHROPIC_API_KEY`; it is
  distinct from the keyless *capture* path. (`src/scorer.rs`,
  hivemind-uuq9.19, 7b64b8c)
- **2-D spectral decision map.** `GET /v1/decisions/map` returns a spectral
  layout (x=time, y=Fiedler eigenmap) backed by `fastembed-rs`
  BGE-small-en-v1.5 semantic embeddings. **Currently SQLite-backend only** —
  the endpoint returns a validation error on the Postgres backend, so the map
  is not yet available on the hosted deployment. (hivemind-plvq, 36519f6 /
  0c34846)
- **`hivemind query recall` CLI.** Wraps search + summarize in one command
  for fast on-demand recall. (M3/ynt5.2, 80d74c0)
- **`recall_decisions` MCP tool.** One-call search + summarize over the MCP
  gateway. (M3/ynt5.1, 825dd9e)
- **TUI summary and compact-graph views.** `'s'` opens a summary panel;
  `'v'` toggles compact-graph mode in `hivemind query`. (hivemind-0o9e,
  4d528ec)

#### MCP and transport
- **HTTP Streamable MCP transport.** MCP tools available over the HTTP
  Streamable transport in addition to stdio, enabling web-based MCP clients.
  (hivemind-iwz9, 8448868)

#### Measurement (M4)
- **Capture-fidelity evaluator and gold corpus.** `benchmarks/fidelity/` —
  36-case gold corpus, schema-ceiling scorecard, and `--ceiling` mode.
  (hivemind-21zi, hivemind-0d2v, 7dc29e2)
- **A/B uplift harness.** `ab-eval` binary compares two classifier
  strategies on the same corpus; Phase 1 scorecard published at
  `benchmarks/fidelity/ab-eval-scorecard-v1-phase1-2026-07-13.txt`.
  (ee1798d / df894c2)
- **Tier 1.5 synthetic corpus** (G1–G3): dispersion-graded cases for
  evaluating classifier robustness across signal density.
  (hivemind-92fs, d56f799)

### Fixed
- Self-host GA polish: removed broken `--agent hivemind-classifier` call in
  `capture.sh`, added batch-capture slash command, corrected non-existent
  remote MCP flags in docs, fixed `hm_sk_live_` → `hm_tk_` token-prefix
  mismatch, added Postgres FTS note in quickstart.
  (hivemind-669v, c87a819)
- Postgres AppState build/startup panic; `extract_ctx` Postgres lookups
  wrapped in `spawn_blocking`. (hivemind-noc9, hivemind-e8zp)
- Release pipeline could publish an incomplete release. `continue-on-error`
  on the build matrix plus an `if: !cancelled()` publish gate let a release
  publish with platform artifacts missing. Publish is now gated on all
  builds succeeding, with an explicit guard that refuses to publish unless
  every expected platform tarball and checksum is present. linux-arm64
  cross-linking (`cannot find -lstdc++`) is fixed by installing the aarch64
  C++ toolchain. (hivemind-ehwd, 375f81e)
- Three `cargo-audit` advisories resolved: crossbeam-epoch, quinn-proto,
  lopdf. (hivemind-7g5o)
- Docker base image bumped to Debian Trixie (glibc 2.40) for `ort`
  pre-built ONNX compatibility. (hivemind-plvq)

### Changed
- **README install snippet.** `HIVEMIND_VERSION` example updated to
  `v0.4.0`.

## v0.3.0 — 2026-06-18 — M3: Layer-3 recall tools over the MCP gateway

HiveMind gains its first Layer-3 read tools for on-demand recall: decision
summarization and graph compactification, alongside richer search. All are
exposed as read-only tools over the authenticated, tenant-isolated MCP gateway,
so agents can recall and condense organizational decision memory on demand
without a local ledger. Layer-3 stays swappable — the write and query layers
remain fully functional without it.

### Added
- **Decision summarization** (`summarize_decisions` MCP tool, Layer-3). Produces
  a text summary of the decisions matching a query/filter, built purely from the
  read layer — no writes, no inference beyond explicit status derivation.
  (`src/summarize.rs`)
- **Graph compactification** (`hivemind_compact_view` MCP tool, Layer-3). A
  `CompactView` query that collapses safely-redundant detail while preserving
  main decisions and their rationale, per the signal/noise semantics in the
  spec. (`src/queries/compact_view.rs`)
- **Compactification specification.** `docs/COMPACTIFICATION_SPEC.md` defines the
  signal/noise rules for graph compaction — what detail is safely droppable
  versus what must always be preserved.
- **Search filter improvements.** HTTP decision search now honors the `source`
  filter and comma-separated `actor_id` values. (`src/api.rs`)
- **MCP gateway end-to-end tests.** `tests/mcp_stdio_e2e.rs` exercises the stdio
  MCP gateway across search, summarize, compact-view, and tenant isolation.

### Deferred to follow-up
- **hivemind-yfbq** (P3): tighten the UBS warning baseline now that
  assertion-heavy integration-test files under `tests/` are exempted via
  `.ubsignore`. Criticals remain gated everywhere; only the warning-count
  baseline is affected.

## v0.2.0 — 2026-06-17 — M2: Shared multi-tenant backend

HiveMind now runs as a hosted multi-tenant service. A single Postgres-backed
process serves multiple tenants with cryptographic token auth and row-level
isolation, reachable over a REST API from CLI, MCP, or any HTTP client.

### Added
- **Postgres ledger + projection.** `PostgresEventLedger` and
  `PostgresGraphView` back the write and read layers in Postgres. Schema
  migrations run at startup. Projection is still rebuildable from the ledger.
  Enabled via the `shared-backend-postgres` feature flag.
- **HTTP REST API** (`/v1/`). A third transport over the same internal commands
  and query functions: `POST /v1/decisions`, `GET /v1/decisions/{id}`,
  `GET /v1/decisions/search`, `GET /v1/decisions/relevant`,
  `POST /v1/ingest`, `POST /v1/tenants`, `GET /v1/health`.
  Bearer token auth is enforced when an API key is configured.
- **Tenant provisioning + Postgres RLS isolation.** `POST /v1/tenants`
  creates a tenant and issues a `hm_tk_<64-hex>` bearer token. Token
  resolution uses a SHA-256 hash stored in `hm_tokens`. Row-level security
  in Postgres prevents cross-tenant event reads at the database layer.
  SQLite dev mode provides header-based tenant scoping for local development.
- **Capture clients: hook + sidecar.** Two autonomous capture surfaces built
  over a shared `ingest-client` core: a commit-hook shipper and a sidecar
  daemon. Both carry actor provenance and route to the same `/v1/ingest`
  endpoint.
- **Haiku classifier for ingest batches.** `POST /v1/ingest` with the
  `shared-backend-postgres` feature routes accepted batches through a
  Claude Haiku call that extracts decision candidates, topic, and confidence
  from conversation turns. Classifier output is stored per-batch.
- **MCP read gateway.** A thin MCP server that wraps the HiveMind HTTP API,
  exposing `search_decisions` and `get_decision` tools to MCP clients
  (Claude Code, Codex, IDE plugins) without a local SQLite dependency.
- **Docker deploy.** `Dockerfile` and `docker-compose.yml` for production
  deployment. `/v1/health` endpoint and Docker healthcheck. Deployment guide
  in `docs/DEPLOYMENT.md`.
- **Multi-tenant test fixtures and integration tests.** Dedicated
  `tests/multi_tenant.rs` suite covering three isolated tenants. HTTP
  integration tests in `tests/api.rs` covering auth, RLS, ingest,
  capture+query round-trip, and supersession.
- **Decision-scoring model locked** (design only; not yet implemented).
  Layer-3, post-PoC 2-axis scoring design recorded in
  `docs/DECISION_SCORING.md` with research basis in
  `docs/DECISION_QUALITY_LITERATURE.md`.

### Architecture docs updated
- `docs/ARCHITECTURE.md` — updated to reflect M2 transport surface and
  Postgres state model.
- `docs/REMOTE_DB.md` — updated to M2 shipped state.
- `docs/AGENT_DECISION_CAPTURE.md` — updated capture flow for HTTP service.
- `docs/MCP_SERVICE_SPLIT.md` — documents MCP gateway over HTTP API.
- `docs/M2_VERIFICATION.md` — e2e verification matrix, all steps passed.

### Deferred to follow-up
- **uuq9.8**: `hivemind migrate` CLI for local-to-remote ledger migration.
  Step 5 of M2 verification is explicitly out of scope for this release.
- **uuq9.19**: Full 2-axis decision scorer (Layer-3, post-PoC). Design is
  locked and documented; implementation is not authorized until after the
  hosted MVP.

## v0.1.0 — 2026-06-15 — M1: Dogfood loop ships

First milestone release. HiveMind — an event-sourced, graph-projected decision
ledger for human governance of agentic decision-making — is now usable end to
end inside its own repository.

### Added
- **Event-sourced decision ledger.** Typed events (decision, option, evidence,
  hypothesis, relation) appended to an authoritative SQLite ledger and projected
  into a graph (Decision, Actor, Evidence, Option, Hypothesis). Status
  (proposed / accepted / contested / superseded) is derived from edges, never
  stored; the ledger is the trust boundary and smart behavior stays out of the
  write path.
- **Capture surfaces.** CLI (`hivemind emit decision.capture`), an MCP server,
  and Claude Code / Codex capture plugins — all thin wrappers over the same
  internal commands, carrying actor provenance (`agent:<tool>:<session>` /
  `human:<id>`) and `source=agent|human`.
- **Deterministic query layer.** `search_decisions` with filters (topic, status,
  actor, source, time, evidence, supersession), full-text search, bounded pages,
  and explicit rank basis. Semantic/vector ranking is intentionally kept out of
  the query path (a layer-3 concern, above the ledger).
- **Dogfood loop (M1).** Concurrent multi-agent writes to a shared ledger,
  repo-local MCP config, plugin actor-prefix conventions, the `docs/DOGFOOD.md`
  operations guide, and foundational VISION / PRINCIPLES / STRATEGY decisions
  seeded into the ledger.
- **Verification recorded in the ledger itself.** The dogfood loop is marked
  operational by two distinct agents capturing real decisions, verified end to
  end — HiveMind used by the team building it.

### Notes
- Storage is local-first SQLite today. A Postgres-backed multi-tenant service is
  the M2 direction (see `docs/REMOTE_DB.md`, `docs/MULTI_TENANCY.md`).
