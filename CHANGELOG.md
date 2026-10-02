# Changelog

All notable changes to HiveMind are documented here.
This project adheres to [Semantic Versioning](https://semver.org/).

## Unreleased

### Breaking changes

- **`score_decision` and `scan_decision_quality` return the quality profile and attention
  findings; the score, the tier and the five deductions are gone.** `score_decision` now
  returns the decision's seven dimensions (framing, alternatives, information, reasoning,
  values_tradeoffs, bias_exposure, calibration), each `assessed` (a level of `none`,
  `partial` or `solid`, the reasons and the node ids behind it) or `not_assessed` (and why),
  plus attention lines and a provenance object whose `line` reads "not yet reviewed by a
  human" when an agent authored the decision and no human accepted it. `scan_decision_quality`
  returns one page of attention findings (a bet past its check date, a premise that was
  superseded or rejected, an assumption or bet that was refuted, evidence nobody re-checked),
  each with a stable `finding_id`, the reason, the node ids and the dimensions it bears on;
  `truncated` and `data.next_cursor` page through them. Both work self-hosted, with no model.
  Removed with no alias: the `score` and `tier` fields, `reasons[].deduction`,
  `contributing_ids`, the `min_tier` argument and `--min-tier` flag, and the `since_event_origin`
  argument on the scan (pass `kinds`, `evidence_window_days`, `limit` and `cursor`, or
  `--kind`, `--evidence-window-days`, `--limit` and `--cursor`). Superseded, premise-stale,
  contested, agent-only and thin-structure signals are no longer graded as quality: they are
  outcome, staleness, status, provenance and Alternatives / Information, shown as such. The
  stdio server, the HTTP endpoint and the CLI share one core, so the three answer the same.
  `hivemind quality-scan` files one ticket per finding (title `[HiveMind] <kind>: <decision>`),
  without a score or tier; `--min-tier` and `--since-event-origin` are gone, `--kind` is new.
  `docs/DECISION_SCORING.md` describes what exists and marks Composite, Confidence and
  Reputation deferred. (hivemind-qo11.5)
- **The "did it hold up" view stops carrying quality signals.** `verify`, `why`
  (`get_decision_neighborhood`), `decision_quality_candidates` and the Markdown decision-log
  export no longer list a `thin_structure` reason: how a decision was made (options weighed,
  what it rests on) is quality, and `score_decision` reports it as Alternatives and
  Information. What stays is what happened to the decision: superseded (by which decision;
  nothing weighs how quickly), a premise that no longer stands, contested. Removed with no
  alias: the `thin_structure` reason kind and its `no_options` / `nothing_declared` fields, the
  `has_options`, `has_evidence` and `grounding_state` fields on each `decision_quality_candidates`
  record, and `signal_breakdown.thin_structure_count` on `analyze_failure_modes`. A decision
  with no options and nothing it rests on now reads "still holds: yes" with no reasons; its
  `rests_on` still says "nothing declared". `analyze_failure_modes` gains `by_condition`:
  failure rates for each quality dimension at each level (`none`, `partial`, `solid`,
  `not_assessed`). A level only sorts decisions into groups; it is never itself a failure.
  (hivemind-qo11.7)
- **The Markdown decision-log export shows each decision's quality profile, and its Outcome
  no longer lists thin structure; `quality-scan` tickets carry the dimension lines.** Every
  decision file from `hivemind export --format markdown` gains a `## Quality profile` section
  between `## Outcome` and `## Provenance`: the seven dimensions of `score_decision`, each as
  a bullet with its level and, beneath it, its reasons with the node ids they rest on, or
  "not assessed" and why, plus the attention lines when there are any. The Outcome section's
  `Reasons` no longer includes `Thin structure: ...` (a decision with no options or nothing
  declared it rests on reads as its Alternatives and Information lines instead); it lists only
  what happened to the decision. Exports written before this hold the old shape until
  re-exported. A `quality-scan` ticket body now names the finding id (`Finding ID`) and lists
  the dimensions the finding bears on (`Dimensions it bears on`), each with its level,
  reasons and ids, or why it was not assessed; the reason heading reads `Why it needs a look`.
  Neither the export nor a ticket contains a score or a tier. Library callers:
  `queries::export_decision_log` takes a fourth argument, `Option<ProfileSection>`, that
  returns each exported decision's section (pass `None` to omit it; the CLI passes
  `quality_profile::decision_log_section`), and `linear::format_issue_title` and
  `format_issue_description` take the scan's `ScanFinding`. (hivemind-qo11.6)
- **A capture under a registered project may use only the topic keys that project has
  declared.** A topic says what a decision is about; left free, every capture invents its
  own keys and recall by topic becomes a lottery. Keys are lowercase kebab (as before), and
  a project's vocabulary grows only when someone says so: a capture's `--declare-topic`
  (MCP `declare_topics`) for a key it uses, or `hivemind project declare-topic <handle>
  <key>...`. The reply lists what a capture declared; a capture that uses an undeclared key
  is refused before anything is written, naming the keys, what the project has and how to
  declare. `hivemind project show` lists a project's keys. A project that already has
  decisions adopts what they use with `hivemind project declare-topic <handle> --in-use`.
  Personal projects and captures with no project have no vocabulary and are unchanged. A new
  ledger event, `project.topic_declared`, records each declaration (upgrade the server
  before writing one); no existing event changes. Captures into a registered project that
  omit `--declare-topic` for a new key are refused, so callers that invent keys need the
  flag. (hivemind-zywz)
- **A capture from a rig the ledger has no project for is refused, not filed under your
  personal project.** With `--project-from-context` (the capture plugins, `hivemind mcp
  --project-from-context`), a session in a Gas City rig that no project in the ledger is
  anchored to is writing to the wrong ledger, which is how a rig's decisions ended up in
  another ledger. The refusal names the rig, the ledger (tenant and local directory or shared
  database, never a credential) and the ways out. A folder marker, a rig anchor or a current
  project still wins first; a supersede inherits the replaced decision's project and is not
  refused. (hivemind-zywz)

### Added

- **`GET /v1/decisions/{id}/possibly-related`: decisions that may be related, labelled inferred,
  never a recorded relation.** Most decisions have no recorded relation to another decision, yet
  every capture carries its topic keys and, for a classifier capture, the one ledger event that
  recorded it out of a conversation. The endpoint offers, for one decision, the decisions
  recorded out of the same conversation first, then decisions that share a topic key only a few
  decisions carry. A topic key carried by more than `max(10, 2% of decisions)` decisions is an
  area tag ("refinery", "ci") and links nothing; the asked decision's keys that were set aside
  come back in `ignored_topic_keys` with how many decisions carry each, so the filter is never
  silent. Items are ranked by same conversation, then by the sum of `1 / (carried_by - 1)` over
  the shared keys, then by id, and each item lists those keys with `carried_by`, so the order
  can be recomputed from the answer. Decisions already joined to it by a recorded `SUPERSEDES`,
  `FOLLOWS_FROM` or `SAME_AS` are left out. The answer carries `layer: "inferred"` and a note
  saying so; it pages with `limit` and `cursor` and says when it is `truncated`. Nothing is
  written, the graph and `GET /v1/graph` gain no edge, and no ledger event or projection
  changes, so nothing needs migrating. "Same conversation" is "same recording event": a
  conversation the classifier recorded in several events appears as several groups. HTTP only for
  now; there is no MCP tool or CLI verb yet. Library callers: `possibly_related::possibly_related`
  (Layer 3) over the new `queries::decision_capture_facts` and `queries::recorded_decision_links`.
  (hivemind-xarm)
- **Attention lists and a per-decision timeline: what is waiting, contested or changed, and
  when each thing happened.** Three lists say what needs a person without anyone going looking,
  each paged with `limit`/`cursor` and saying when it is `truncated`: **waiting**
  (`get_waiting_requests`, now also an MCP tool and `GET /v1/attention/waiting`) lists open asks
  with no answering decision, oldest first, with `asked_at` and who asked; **contested**
  (`hivemind query get_contested_decisions`, MCP `get_contested_decisions`,
  `GET /v1/attention/contested`) lists decisions accepted and rejected by different actors
  (naming both sides) and accepted decisions whose answers to one question choose differently
  (each naming the other), oldest first, each with `asked_at` and `decided_at`; **changed** (`hivemind query get_changed_decisions`,
  MCP `get_changed_decisions`, `GET /v1/attention/changed`) lists decisions superseded,
  retitled, moved or left without a premise in a window (`--since`, default the last 7 days,
  echoed in the reply), most recently changed first, each with `asked_at`, `decided_at` and its
  dated changes cited by ledger event. `hivemind why`, MCP `get_decision_neighborhood` and `GET /v1/decisions/why` gain
  `data.timeline` (and `GET /v1/decisions/{id}/timeline` reads it alone): the decision's dated
  story, oldest first in ledger order, asked, recorded, accepted or rejected, superseded,
  retitled, moved, and the moment a premise it rested on stopped standing (a refuted hypothesis,
  a superseded or rejected decision it follows from, written only for decisions that already
  rested on it then), with `asked_at`, `decided_at` and, when someone asked first,
  `asked_to_decided_seconds`; `why --summary` prints it as a `timeline:` block. Speed is the
  waiting list plus that one duration per decision: no averages, no per-person figures, no
  grading; a decision nobody asked about has no ask time. An ask records who asked, not whom, so
  the waiting list cannot say who is being waited on. No ledger event changes and nothing needs
  migrating; `docs/ATTENTION_LISTS.md` holds the rules. Library callers: `DecisionStandings`
  also reads who rejected a decision (`rejecters_of`); `NeighborhoodView` gains an optional
  `timeline` that only a caller holding the ledger fills. (hivemind-bbnw.7)
- **`hivemind ask` records that you are explicitly asking a question, before any decision
  answers it.** `hivemind ask "<question text>"` and MCP `request_decision` write a new
  `question.asked` event (envelope `ts`/`actor_id`/`source_ref` are the ask's time, asker and
  where — never back-dated), resolving the question exactly like a capture's `--question`: a
  match on the normalized text (lowercase, spaces collapsed, trailing punctuation dropped) is
  reused, otherwise `question.recorded` creates the node. Unlike answering, asking is never
  suppressed as a duplicate: the same question can be asked more than once, each its own
  outstanding request (new `Ask` node, `ASK_FOR`/`ASKED_BY` relations — see
  `docs/GRAPH_CONTRACT.md`). `emit decision.capture --answers <request id>` (MCP
  `capture_decision` with `answers`) resolves an existing request to its question and links the
  decision to it exactly as `--question <text>` would, without repeating the words; refused
  when the request does not exist, or together with `--question`. Distinct from `ground
  --answers`, which takes the question's own text, not a request id. `hivemind query
  get_waiting_requests` lists open requests (no answering decision yet) oldest first: request
  id, question, asked_at, who asked. `hivemind why` (`DecisionBrief`, the graph query layer,
  and the MCP/HTTP `get_decision_neighborhood` it feeds) on an answered decision now also
  carries `asked_at` (the earliest explicit ask for its question, when there was one) beside the
  existing `occurred_at` (the decision's own timestamp — unchanged, still the JSON field name);
  the CLI text summary labels the two lines `asked_at:` and `answered_at:` (was `when:`).
  `verify` is unaffected — asked/answered times are a `why` feature.
  Deliberately out of scope for this pass, left for a follow-up: `--project` on `ask` (the
  underlying `Question` node has never been project-scoped, matching hivemind-zdsh.16), an MCP
  tool for the waiting list (the CLI query is the only required surface), and the full
  waiting/contested/changed attention lists with per-decision timelines (hivemind-bbnw.7). This
  reuses none of `decision.requested`/`DecisionRequest`/`NodeKind::DecisionRequest`: that
  existing, unused scaffolding belongs to a different, still-undecided design
  (`docs/HUMAN_DECISION_BLOCKER_NOTIFICATIONS.md`) with its own required fields (`priority`,
  `authority_class`, `client_request_id`) that do not fit an explicit ask; overloading it would
  have conflated two concepts under one event type. (hivemind-bbnw.4)
- **The Claude Code plugin records an agent's question to you as an ask, and your answer as a
  decision that answers it.** `hivemind-capture` now ships hooks on the `AskUserQuestion` tool
  (`plugins/hivemind-capture/hooks/hooks.json`). `PreToolUse` writes one ask per question at the
  moment of the tool call (MCP `request_decision`; the event's own time is `asked_at`, never a
  guess). `PostToolUse` writes one decision per answered question: recorded by the agent,
  decided by you (`human:<git email>`, or `HIVEMIND_HUMAN_ACTOR`), the offered options, the one
  you picked, and any words of your own quoted verbatim. Several picks in a multi-select, or your
  own words, become one combined option (`Search + Export + Other (own words)`) and what you
  picked into it is not listed as turned down. The decision names the same question as the ask,
  so `hivemind why` shows `asked_at` beside `answered_at` and the request leaves
  `get_waiting_requests`; the hooks share no state. The note you write on your pick is the
  rationale, word for word (a note too short to read on its own is quoted instead); only with
  neither a note nor words of your own does it say no reasons were given. It rests on a bet with
  nothing declared, because nothing was stated with the answer. The answer is filed under the
  slug of the question's header; when a registered project has not declared that topic (a
  project accepts only declared ones), the hook writes it once more under the fixed key
  `claude-code-question` and declares only that key, so a project's vocabulary grows by at most
  one key from these hooks, never one per question, and your answer is recorded in every
  checkout (a personal or unregistered one has no vocabulary and declares nothing). Only that
  refusal is retried; both attempts are in `hook.log`. A question you decline
  or never answer stays waiting; nothing is recorded when nobody asked (an agent deciding alone,
  you deciding unprompted, an import). Writes go to the local ledger through `hivemind mcp`, or
  to `POST /mcp` of the server named by `HIVEMIND_API_URL` (bearer token `HIVEMIND_API_KEY`), so
  a hosted deployment works the same way. The hooks never get in the agent's way: a failed write
  is logged (`hook.log`) and the question still appears; `HIVEMIND_ASK_HOOK_DISABLE=1` turns
  them off. Needs `python3` and a CLI or server with the ask verb. Tests replay the hook
  payloads of a recorded Claude Code session
  (`tests/fixtures/claude_code/ask_user_question/`). The `hivemind-capture` skill tells agents
  not to capture the same answer again. (hivemind-bbnw.5)
- **`GET /v1/graph` says when each decision was made.** Every `decisions[]` entry and every
  `nodes[]` entry of `kind: "Decision"` now carries `decided_at`, the `decision.proposed`
  capture event's timestamp in ISO-8601 UTC (`null` only for an event predating the ledger's
  timestamp backfill), so a whole-graph view can show a decision's date without a second
  lookup. Additive: no existing key changes. Documented in `docs/GRAPH_CONTRACT.md`.
  (hivemind-zbd0)
- **A decision's link no longer moves.** Every `decisions[]` entry, every `nodes[]` entry of
  `kind: "Decision"`, and the decision brief (`GET /v1/decisions/verify`) now carry `slug`: the
  decision's title, kebab-cased, with a growing id-tail suffix for a later decision whose title
  slugs to the same thing. The server assigns it once, when `decision.proposed` is first
  projected (ledger order, so replay is deterministic); nothing projected afterwards touches
  it, so a retitle or any other annotation leaves a decision's link unchanged. Additive: no
  existing key changes, no new event. Documented in `docs/GRAPH_CONTRACT.md`. (hivemind-nidp)
- **Importers keep the source's own time instead of import time, and record an explicit ask as
  `question.asked`.** Every `Commands`-driven write can carry an explicit event `ts`
  (`CommandContext::event_ts`) in place of wall-clock "now". The Slack thread importer stamps
  the events it writes with the deciding message's own time (the `Chosen:`/`Chose:` message,
  else the `Decision:` message), and the local document importer honors an optional
  `ts:`/`decided:`/`decided-at:` block marker, falling back to the file's last-modified time when
  the block names none. That fallback is "last modified", not "decided": a git checkout or a copy
  resets it. The same block parser reads a `Decision:` block pasted from a git commit trailer or
  a tracker/beads ticket body, so a `ts:` in that block carries the commit's or ticket's own
  time; none of these is a separate importer. An ask has one home, the `question.asked` event
  from `hivemind ask`: the Slack thread importer writes one, at the root message's own time and
  by the root's author, only when the root is a different message from the one carrying the
  decision **and** is recognisably a question by a deterministic test (it ends in `?`, or
  carries a `Question:`, `Ask:` or `Decision needed:` marker); the decision then answers it, as
  `capture --answers` does. A decision posted top-level, a root that states the decision and a
  root that does not visibly ask get their own time and no ask, so the timeline reads "asked at:
  not recorded" rather than a guess. Live Slack app captures see only their own message, so they
  write no ask; documents never do. `ingest.batch_received` turns can carry an optional `ts`
  (Claude Code and Codex JSONL logs already have one); classification does not consume it yet.
  `docs/TEXT_IMPORT_AND_DIFF_SEMANTICS.md`, `docs/SLACK_APP.md` and
  `docs/CAPTURE_CLASSIFIER.md` describe the rule per source. (hivemind-bbnw.6)
- **A decision the transcript classifier derives is recorded at its own turn's time, and an
  explicit decision-request turn writes `question.asked`.** A classified capture can name the
  turn it came from (`source_turn_id`, the turn id the classifier now sees in each turn's
  header) and, on a `decision` or `decision-request`, the question in the words it was asked in
  (`question`). The write path checks the turn is one of the batches the classification covers
  (refusing the classification otherwise), stores that turn's own time on the capture as
  `source_ts` (never taken from the caller), and the projector records the decision at that
  time, so `why` shows when it was decided instead of when the transcript was classified. A
  capture that names no turn, or whose turn carries no time, keeps the classification's time.
  A `decision-request` that states a question and names a turn with a time writes one
  `question.asked` at that turn's time, by the actor the classifier named on the request (with
  none named, a request in an assistant's turn is credited to whoever submitted the batch, that
  agent having spoken it, and a request in a user's turn writes no ask, since the batch records
  no human and a human's question is never credited to an agent); a later decision that states
  the same question links to it with
  `ANSWERS`, matched on the normalized text exactly as `capture --question` is. No ask is
  written for an agent deciding alone, for a request with no question, or for a request turn
  with no time (the timeline reads "asked at: not recorded"), and a re-ingested transcript
  writes the same ask once. `source_turn_id`, `source_ts` and `question` are new optional
  fields of `ingest.batch_classified` captures: a plugin that sends them to an older server is
  refused, so upgrade the server first. The shipped hooks already send each turn's `ts`.
  `docs/CAPTURE_CLASSIFIER.md` and `docs/AGENT_DECISION_CAPTURE.md` describe the rule.
  (hivemind-bbnw.8)
- **A model's assessment of a decision can be recorded and is shown beside the quality
  floors.** `decision.scored` gains a second payload version (`schema_version: 2`), keyed by
  the decision's id (a proposed decision or a classified capture): all seven dimensions, each
  either assessed (a level of `none`, `partial` or `solid`, an explanation and, at `partial`
  or `solid`, the verbatim passage of the decision's own text it rests on; a `none` answer may
  leave the quote out, since an absence cannot be quoted) or not assessed (and why), with the
  model and prompt version. The write path refuses, and records nothing, when a quote does not
  occur in the decision's recorded text or a `partial` or `solid` answer gives none.
  `score_decision` adds `model_assessment` beside the floors
  (only when one exists; no floor changes because of it), and the CLI summary and the Markdown
  export show it. Existing `decision.scored` events (the float scores) are unchanged in the
  ledger and keep replaying; the profile never shows them. Nothing produces a version-2
  assessment yet. (hivemind-qo11.8)
- **Two producers now write a model's assessment; the version-1 write path is retired.** The
  background scorer (`src/scorer.rs`, `ANTHROPIC_API_KEY`) and the keyless `emit decision.scored`
  path write `decision.scored` schema version 2 exclusively, through the same
  `Commands::record_decision_assessed` validator. The scorer now also assesses decisions
  captured directly (`decision.proposed`/`decision.capture`), not only classifier-extracted
  captures, and no longer double-assesses a decision that already has a version-2 event. Its
  prompt asks the model to assess Framing and Values / Tradeoffs — the two dimensions with no
  floor beyond "a question was recorded" — and to enrich the other five only where it has real
  grounds, leaving the rest `not_assessed`; there is no 0.5 placeholder. `emit decision.scored`
  takes the scores file `{"dimensions": {...}}` (the same shape) and names its target by
  `--decision-id` (a decision captured directly) or `--batch-id`/`--capture-index` (a classifier
  capture); `--scorer-model`/`--weight-version` are gone, replaced by `--model` and the required
  `--prompt-version`. Nothing changes when `ANTHROPIC_API_KEY` is absent: the worker does not
  start, and the profile stays complete on floors alone. Version-1 `decision.scored` events
  already in a ledger keep validating, replaying and projecting unchanged; nothing new writes
  that shape. (hivemind-qo11.9)
- **`get_suggestions` returns what needs a look and has not been dealt with.** A new MCP tool
  on the stdio server and the HTTP endpoint, and `hivemind query get_suggestions`: the
  attention findings of `scan_decision_quality`, less the ones that have been acknowledged
  (matched by `finding_id`). `exclude_acknowledged` (`--exclude-acknowledged` on the CLI) is
  true by default; false returns every finding. The page is filled from the findings that
  remain, so it holds `limit` findings whenever that many remain and `truncated` is exact. A
  finding whose basis changed has a new `finding_id`, so an old acknowledgement never hides
  it. Same arguments, response shape and refusals as `scan_decision_quality`; no score, no
  tier. (hivemind-m306.4.1)
- **`acknowledge_suggestion` records that a finding was looked at, and `get_suggestions` then
  leaves it out.** A new MCP tool on the stdio server and the HTTP endpoint: pass the
  finding's `finding_id` and `decision_id` (and optionally `action`: `seen`, `acted` or
  `dismissed`, default `seen`; `channel`, default `mcp`). It appends two attributed events: a
  new event type `suggestion.surfaced` (this finding, shown to the acting actor over this
  channel) and a `notification.acknowledged` naming it. `get_suggestions` with
  `exclude_acknowledged` (the default) leaves the finding out until its basis changes and its
  `finding_id` changes with it; `exclude_acknowledged: false` and `scan_decision_quality` still
  show it. Acknowledgement is by finding, not by actor, and an acknowledgement with a
  `snooze_until` hides the finding only until then. A surfaced finding projects to a
  `Notification` node with no blocker, so the blocker queries do not see it.
  **Schema:** `notification.acknowledged` gains an optional `action` (`seen`, `acted`,
  `dismissed`) and `suggestion.surfaced` is a new `schemas/v0` event. Both are additive: no
  migration, every acknowledgement already in a ledger stays valid and replays unchanged, and
  Kuzu graphs are rebuilt from the ledger. **Upgrade the server before any client writes the
  new event:** a binary from before this change cannot read a ledger that holds a
  `suggestion.surfaced`. (hivemind-m306.4.2)
- **A decision answers a question, and decisions that answer the same one are findable.**
  `--question "<one line>"` on `emit decision.capture` (MCP `question`) now names the
  question the decision answers, and it no longer needs a `--quote`: only a quote needs its
  question. Two captures whose question is equal after lowercasing, collapsing spaces and
  dropping trailing punctuation share one question node (a new `question.recorded` event and an
  `ANSWERS` relation; the capture reply gains `question_id`). The match is exact, with no ranking
  and no model. `query verify` / `why` print `answers: <question>` and `also answered by: <title>
  [status]`; two accepted, non-superseded decisions that chose different options are flagged on
  both as `conflicting_answer` (attention, not staleness: neither is marked stale, nothing is
  resolved); `query situational` shows a newer accepted answer to the question of a decision it
  matched. `hivemind ground "<decision>" --answers "<question>"` (MCP `ground_decision` with
  `answers`) links a decision captured without one, attributed to whoever runs it. Existing
  ledgers replay unchanged: a decision recorded with question text and no node keeps the text and
  gets no node. (hivemind-zdsh.16)
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
- **A decision captured before the title cap can be given a short name.** `hivemind retitle`
  (MCP `retitle_decision`), found by description or `--decision`, appends a new
  `decision.retitled` event instead of rewriting anything: the projection shows the new title,
  the node keeps `former_title`, and the event carries `from`/`to` and an optional `reason`.
  The new title obeys the same cap capture enforces; an unknown decision, a stale `from`, or a
  title that is already current writes nothing, and no event is appended. Reversible: retitling
  back is just another recorded retitle. `recent_decisions` / `decisions_added_since` /
  `decisions_changed_since` gain their own `title_changed` history line (mirrors
  `decision.moved`'s `project_moved`), and `why` is unaffected. (hivemind-ydmp)

### Changed

- **`scan_misfiled_decisions` says where each decision is filed and where it belongs.** Each
  candidate carries its `project`; `--project` limits the report to decisions filed exactly
  under one project (an unregistered handle is refused, never an empty report) and
  `--move-to <project>` names the destination, skips decisions already there and gives each
  text row the `hivemind move --decision <id> --to <project>` that moves it. Still a report:
  it moves nothing. (hivemind-zywz)

### Fixed

- **A decision that several sessions restate is one decision to `recall` and `why`, not
  three records that make `why` ambiguous.** The classifier now compares each decision it
  extracts with the closest recorded decisions and, when it states one again, records it with
  `restates_id`; the capture projects a `SAME_AS` link to the decision it restates. `recall`
  and the description resolvers behind `why`, `verify`, `chain` and `compact-view` show linked
  records as one: the earliest record that matches the words, with the others as
  `also_recorded_as` (`{decision_id, title}`; an `also recorded as:` line in `--summary`).
  Records with no link between them stay separate, so a description matching two is still
  ambiguous. A restating capture classified from turns with the same newest source time as
  the decision it restates (a transcript sent twice) is not recorded a second time; the
  reply's `restated` lists each restating capture as `linked` or `deduplicated`. The capture
  hook now sends each turn's own time (`ts`) so this can tell a re-sent transcript from a
  decision made again. For decisions recorded before this, `hivemind restatements propose`
  lists the links to approve (title-word overlap, with the shared words) and
  `restatements apply [--all | --link LATER=EARLIER]` records them; nothing is deleted or
  rewritten. `restates_id` is a new field of `ingest.batch_classified` captures: upgrade the
  server before a client that sends it. (hivemind-83cj)
- **One malformed model-assessment row no longer makes every read of the ledger fail.** A
  `decision.scored` event of schema version 2 whose payload cannot be read (a hand-injected
  row, a buggy or old producer, a partial import) used to abort the projection, so `query
  recall`, `why`, `verify`, the CLI summary, the Markdown export, the MCP stdio server and
  `/mcp` all refused to answer. Readers now skip an assessment (and a `decision.scored` or
  `decision.metadata_derived` row of any shape) that fails validation: the decision it names
  shows what its own events say (the floors of `score_decision`, no `model_assessment`), and
  a readable assessment before or after it still lands. The skip is never silent: `query`,
  `digest`, `export` and `quality-scan` on the CLI, and every read tool on the MCP stdio
  server and `/mcp`, carry a one-line notice naming the rows, for example `1 assessment row
  could not be read and was skipped (ledger event 57: payload does not match event type
  DecisionScored: ...); answers leave it out`: a `notice` key beside `data` in `--json` and in
  the MCP result, a final `notice:` line in `--summary` output. The server logs one warning per
  replay with the count and the ledger offsets. An unreadable assessment does not count as
  assessed either, so the background scorer still assesses that decision. The write path is
  unchanged: it still refuses a malformed assessment before anything is appended, and a
  malformed event of any other kind
  (the ones the graph is built from) still fails the read. There is no `doctor` command; the
  notice is where the bad rows are listed. (hivemind-qo11.10)
- **The Claude Code and Codex capture plugins work with the latest release binary.** The
  marketplace serves the plugins from `master` while people install the release binary, so a
  plugin change that passed a flag only `master` has (`--project-from-context`) broke every
  stranger's install: the MCP server closed at start and `capture.sh` exited 2 with nothing
  written. The MCP server config, `capture.sh`, the AskUserQuestion hooks and the
  hivemind-context `supersede` verb now pass such a flag only when the installed CLI lists it
  in its `--help` (an older CLI files the decision under the personal project), the hook
  writes an answer without its question for a release that takes a question only beside a
  quote, and hivemind-context `ground` says the release has no such command. CI runs the
  plugins against the latest release binary (`scripts/check_plugin_against_release.py`), so a
  plugin change that needs a newer CLI fails there. Each surface also names its own tool: the
  Codex bundle starts its own MCP config (`.codex-plugin/mcp.json`, `--agent-tool codex`)
  instead of the Claude plugin's, which filed Codex MCP captures as `agent:claude`, and the
  shared skill's direct CLI form names `claude` or `codex` instead of always `codex`. The
  plugin manifests are 0.1.1 so installed copies update. (hivemind-cxqd)
- **A decision the classifier captured from a transcript no longer reads with no time.** Every
  classified decision now carries the time of its batch as `occurred_at`, so `why`, `verify`,
  `GET /v1/decisions/why` and `decided_at` on `GET /v1/graph` say when. Who decided is still
  only what the classifier named as acceptor or rejecter: a capture that names none stays
  `proposed`, even when a human is credited as the one who proposed it. The change is in how a
  classified batch projects, so a ledger already holding captures reads right after the graph
  is rebuilt from it; no event is rewritten. (hivemind-s0ra)
- **`why`, `verify`, `chain` and `compact-view` answer a question `recall` answers, in one
  call.** `recall` returned a decision for "how do we keep links to a decision page stable
  across browsers?" while `why` said "no decision matches that description", and `verify` on
  "is the product still called Upheld" stopped at a "close" list of one that needed a second
  call with `--pick`. These verbs only read, so they now match at the bar `recall` uses (at
  least half of the words, not more than half), and when the close candidates that are left
  are not tied, the closest one is the answer, with a `close match:` line ahead of
  `--summary` output and `close_match: {decision_id, title, missing_terms}` beside `data` in
  `--json`, the HTTP `why`/`verify` routes and the MCP `get_decision_neighborhood`,
  `get_decision_outcome`, `get_supersession_chain` and `get_compact_view`. The closest is the
  one that lacks the fewest words; between candidates that lack the same number, it is the
  one whose title or topic keys carry more of the words it did match, so the decision named
  "Name the product Upheld" beats a decision that only says "product" and "called" somewhere
  in a long rationale for "is the product still called Upheld". `recall` orders its close
  matches the same way, so it and `why` name the same decision first. Candidates equally
  close on both counts are still listed, and a decision that shares only one word with the
  question is listed, never answered with. Verbs that write
  (`disagree`, `supersede`, `move`, `retitle`, `ground`, and a grounding premise named by
  description) are unchanged: more than half of the words, at least two, and a close
  candidate is listed, never picked. (hivemind-3lko)

- **An attention finding names every decision, premise, bet, assumption and evidence item it
  is about in words, not by a bare id.** `scan_decision_quality`, `get_suggestions` and
  `hivemind quality-scan` named findings only by id: `reason` read "the decision it follows
  from, decision-9455…, was superseded by decision-72e8…", and a finding carried no title at
  all (`finding_id`, `decision_id`, `kind`, `basis_at`, `node_ids`, `reason` were the whole
  shape), so a reader had to look each id up before knowing what the finding was about. Every
  finding now carries `decision_title` (the flagged decision's own title, falling back to its
  id when the record has none), and `reason` quotes the title, statement or content of every
  other node it names — a prior decision by its title, a bet or assumption by its statement, an
  evidence item by its content, clipped to 200 characters — instead of the raw id: "the decision
  it follows from, 'Use the old queue', was superseded by 'Use the new queue' on 2026-08-01".
  The ids (`decision_id`, `node_ids`) stay in the finding as handles for a follow-up call, never
  as the only way to say what the finding is about. The stdio server, the HTTP endpoint and the
  CLI carry the change identically. A `quality-scan` ticket body now opens with the decision's
  title, not only its id, so it reads without looking anything up; `linear::format_issue_title`
  drops its now-redundant `decision_title: Option<&str>` argument and reads the finding's own.
  Small fix alongside: `evidence_not_rechecked`'s reason read "more than 1 days ago" for a
  one-day window; it now reads "more than 1 day ago". (hivemind-uag0)
- **`recall` answers a plain question in the asker's own words.** It said "No decisions found"
  to questions like "how does the decision page get the brief it shows?" because every word
  that was not a question word had to appear in a decision exactly as written (`shows`, not
  `show`), while `why` found the same decision. `recall` now matches like `why` (a word also
  matches its inflections) and keeps a decision that matches at least half of the words, after
  the ones that match them all, fewest missing words first. Each such close match carries
  `missing_terms` (`ranked.items[].missing_terms`; `missing=` in `--summary`; a `Close matches`
  line in the digest) so a partial match never reads as a full one. "What did we decide about
  sign-in and pricing" now finds the sign-in decision and the pricing decision. A question
  that no decision shares half its words with is still an empty answer, and `search` still
  needs every word. The stdio server, the HTTP endpoint and the CLI share the change.
  (hivemind-j1q3)
- **A replacement of a moved decision is filed where the decision was moved, not in the
  replacer's personal project.** `hivemind supersede`, `supersede_decision`, the REST supersede
  and the `review` supersede inherit the old decision's project when none is named, but read it
  from the decision's first proposal and so ignored a later `hivemind move`. A decision moved
  to a shared project (say `ui`) was replaced into the replacer's personal project, against
  what `--help` and the plugin README say. The replacement is now filed where the old decision
  is now, and the same holds when `--project-from-context` finds nothing. It records
  `project_source` `inherited`, a new value: the replacement took its project from the decision
  it replaces, so it is neither `stated` nor a move. (A replacement used to copy the old
  decision's own source, `rig` or `folder_marker`, which said how the old decision was filed,
  not the replacement.) A caller cannot claim `inherited` next to a `--project`, as with `moved`
  and `personal_fallback`. A decision that sits in a personal project is replaced into the
  replacer's own personal project, since a personal address belongs to one actor and cannot be
  stated or moved into by anyone else; `--help` and the plugin README now say so. Replacements
  already on the ledger are not moved: `hivemind move` fixes one. (hivemind-9lzi)
- **`why`, `verify`, the recall digest and the export show options as words, not slugs and
  letter codes.** An option recorded as `name-a-upheld` read `chose: name-a-upheld` and
  `rejected: name-b-standing, name-c-decisis`, with the answer letter inside the label, and
  options from before labels existed read as the raw id or the id's slug. Options are now shown
  as `Upheld`, `Standing` and `Decisis`: a label that is a slug (lowercase words joined by `-` or
  `_`) is turned into words, acronyms keep their capitals (`direct-cli` reads `Direct CLI`; MCP,
  CLI, API, ADR, UI, URL, HTTP, JSON, SQL and CI), and when every option of a decision is lettered
  `a`, `b`, `c`... the shared stem and the letter are dropped. A label that is already words
  (`Direct CLI`, `sqlite`) is shown as recorded, and an option whose only record is an id with no
  words in it stays as that id. The words are a reading of the record, and the record stays in
  view: the Option node keeps the recorded text as `recorded_label`, and where it reads
  differently `why` prints `chose: Upheld (recorded as: name-a-upheld)`, `why --json` and the
  brief carry `recorded_as` on each option, the decision-log export writes `recorded as:` beside
  the option, and `GET /v1/graph` Option nodes carry `recorded_as` for a page to show. The ledger
  is unchanged: the words are derived each time it is replayed, so existing ledgers read this way
  with no migration step. The capture skill, its slash commands, the README, the CLI `--options`
  help and the MCP `capture_decision` schema now teach short human labels (`--options "Direct
  CLI,MCP server" --chose "Direct CLI"`) instead of `--options direct-cli,mcp`, and `hivemind
  quickstart` records `Local ledger` and `Spreadsheet`. New captures that still use slugs are not
  refused; they read as words. (hivemind-hk5z)
- **A capture refused for its project leaves nothing behind.** `decision.capture` and
  `capture_decision` checked the stated project (registered, no reserved `personal:` prefix, a
  source a caller may claim) only after they had recorded the evidence, assumptions and bet named
  on the same call, so a refusal left those as unattached nodes and every retry added more. The
  project is now checked before the first write, and a refused capture appends nothing. Events
  already written that way stay in the ledger; they are unattached nodes, not part of any
  decision. (hivemind-s15q.20)
- **A supersede that says which option was chosen no longer leaves the replacement "not yet
  decided".** `hivemind supersede --chose <option>`, `supersede_decision` with
  `chosen_option_label`, the REST supersede and the interactive `review` supersede recorded the
  replacement as a proposal nobody had decided, so the only live answer on the question read
  `proposed`, and every decision that followed from the old one was told it "was superseded by"
  something undecided. A chosen option now means the replacement was already decided, as it
  does for `emit decision.capture`: it is accepted from the acting actor right after the
  supersession is recorded, and `why` shows who decided. `--still-proposed` (`still_proposed`
  on MCP and REST) keeps a genuine open recommendation at `proposed`. A supersede that names no
  chosen option decides nothing and still leaves the replacement `proposed`. Supersedes already
  on the ledger are not rewritten: no acceptance is invented for a person after the fact.
  (hivemind-k7o9)
- **Six tools that `POST /mcp` listed now run there.** `recent_decisions`,
  `decision_quality_candidates`, `get_decision_context`, `decision_context_candidates`,
  `scan_misfiled_decisions` and `analyze_failure_modes` were in the HTTP endpoint's `tools/list`
  but answered a call with an `unknown tool` error; only the stdio server ran them. They now
  run through the same core as the stdio server, so a call takes the same arguments, is refused
  with the same messages and returns the same response on both. Nothing changes on stdio.
  (hivemind-imp1.1)
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
- **`scripts/cell-update.sh` keeps the update.** It restarted the cell on the sha-tagged image
  but named that image only for its own `docker compose up`, so a systemd timer or cron job that
  keeps the cell up with `docker compose up -d` recreated the container on `:latest` at its next
  run, and the update quietly undid itself. The script now writes `HIVEMIND_IMAGE=hivemind:<sha>`
  into the compose project's `.env` before the restart and keeps it once the new container is
  healthy and `/v1/version` reports the sha; a failed update puts the file back as it was, so a
  reconciler returns the cell to the previous image. An existing `.env` keeps its mode and every
  other line, and a new one is created owner-only. `HIVEMIND_COMPOSE_FILE` now takes several
  compose files, colon-separated, so a cell with an override file is restarted with all of them,
  and `HIVEMIND_ENV_FILE` names an env file other than the `.env` next to the first compose
  file. Documented in `docs/SELF_HOSTING.md`. (hivemind-2mgj)

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
