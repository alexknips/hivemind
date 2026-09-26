# Slack App

HiveMind includes a Slack app surface for human decision capture and
read-only decision queries. The app keeps Slack as an integration layer: Slack
requests are queued, drained into the normal HiveMind command layer, and query
responses are rendered from deterministic read APIs.

Everything below the "HTTP Front Door" section describes the CLI-driven,
local-first surface, still useful for local development and for enqueueing
captures by hand. The HTTP front door section describes making the same
surface reachable from a real Slack workspace over the network.

## HTTP Front Door

`hivemind serve` exposes four endpoints under `/v1/slack/*` so a real Slack
workspace can reach a running HiveMind server directly — no manual CLI step
per event.

| Route | Purpose |
|---|---|
| `POST /v1/slack/events` | Events API: `url_verification` handshake, `app_mention` / `message.channels` / `reaction_added` |
| `POST /v1/slack/commands` | Slash commands (`/hivemind ...`), routed to the same `handle_slack_command` the CLI uses |
| `POST /v1/slack/interactivity` | The "Capture this thread as a decision" message shortcut and its capture modal |
| `GET  /v1/slack/oauth/callback` | OAuth code exchange + workspace install, so a workspace installs without touching a terminal |

These routes authenticate every request via **Slack's own request
signature** (`X-Slack-Signature` / `X-Slack-Request-Timestamp`), never the
bearer-token / WorkOS-JWT path the rest of `/v1/*` uses. Configure:

- `HIVEMIND_SLACK_SIGNING_SECRET` — the Slack app's signing secret (Slack's
  "Basic Information" page). Verifies `url_verification` (which carries no
  `team_id` to look an install up by) and is copied into every workspace
  installed via OAuth.
- `HIVEMIND_SLACK_CLIENT_ID` / `HIVEMIND_SLACK_CLIENT_SECRET` — required only
  for the OAuth callback's code exchange.
- `HIVEMIND_SLACK_API_BASE_URL` — optional; the root of the Slack Web API the
  server calls back into (`https://slack.com/api` when unset). Only needed to
  point those outbound calls somewhere else, such as a stand-in in tests.

Set the manifest's slash-command URL, Interactivity Request URL, Event
Subscriptions URL and OAuth Redirect URL to
`https://<your-host>/v1/slack/commands`,
`https://<your-host>/v1/slack/interactivity`,
`https://<your-host>/v1/slack/events`, and
`https://<your-host>/v1/slack/oauth/callback` respectively:

```bash
cargo run -- --json slack-app manifest \
  --request-url https://your-host/v1/slack/commands \
  --interactivity-url https://your-host/v1/slack/interactivity \
  --event-url https://your-host/v1/slack/events \
  --redirect-url https://your-host/v1/slack/oauth/callback
```

`--interactivity-url` and `--event-url` default to `--request-url`, which is
what the local-first flow below wants: one tunnel URL for everything.

### Multi-tenant

Each Slack workspace (`team_id`) maps 1:1 onto its own HiveMind tenant
(`TenantId::new(team_id)`). The OAuth callback provisions this tenant
automatically (SQLite: `tenants` row; Postgres: `hm_tenants` row + an unused
initial token) on install, so no separate `hivemind tenant create` step is
needed for OAuth-installed workspaces. A `slack-app install` done by hand
(the local-first flow above) still needs a matching
`hivemind tenant create <team_id>` first — the events/commands routes 404
through the same `ensure_known_tenant` gate the bearer-token API path uses
for an unregistered tenant.

This is the seam that keeps a request signed by workspace A from ever
writing into workspace B's ledger: `team_id` is only trusted once the
request's signature has been verified against *that* workspace's stored
signing secret, and every ledger operation after that is pinned to
`team_id`'s own tenant.

### Ack-fast, drain-async

Neither `POST /v1/slack/events` nor `POST /v1/slack/interactivity` writes to
the ledger inline — they enqueue onto the same `SlackAppStore` capture queue
the CLI's `slack-app enqueue-capture` uses, and answer within Slack's
3-second ack window. A background task (started once, from `hivemind serve`)
polls the queue and drains it into each capture's own tenant ledger every 15
seconds.

`app_mention` and `message.channels` events are auto-captured only when the
message text carries the same `Decision:`/`Rationale:`/`Options:` markers
`hivemind ingest slack-thread` already parses (see "Capture Queue" below) —
no LLM inference happens on the ingest path, per AGENTS.md's three-layer
separation. A message without those markers is acknowledged and ignored.

### Capturing from a message: the shortcut

The **Capture this thread as a decision** message shortcut (the `⋯` menu on
any message) opens a modal for the message it was invoked on:

| Input | Required | Notes |
|---|---|---|
| Decision | yes | the decision's title |
| Rationale | yes | why this, over the alternatives |
| Options considered | yes | comma- or `|`-separated |
| Chosen option | no | must match one of the options (case-insensitively); empty leaves the decision `proposed` |
| Topics | no | comma- or `|`-separated; defaults to `slack` |

Submitting queues a capture attributed to the Slack user who submitted (mapped
through the install's `actor_mappings` when one exists). The selected
message — its author, timestamp and text — becomes the decision's evidence.
Input the capture cannot accept (a missing rationale, a chosen option that is
not among the options) keeps the modal open with a message next to the
offending field; nothing is dropped silently. A message longer than a Slack
modal can carry (about 2.5k characters) is cut with an explicit
`[truncated: ...]` marker in the evidence, never silently.

Both halves need the Slack Web API: opening the modal calls `views.open` with
the shortcut's `trigger_id` (good for about three seconds) and the install's
bot token. If Slack rejects that call, the endpoint answers `500`, so Slack
tells the user the shortcut failed instead of showing nothing.

`block_actions`, `view_closed` and any shortcut this app does not define are
acknowledged and ignored — the capture modal has no interactive components.

### Capturing with a reaction

Adding the install's `reaction_emoji` (default `:hivemind:`) to a message
captures it, when the message carries the `Decision:`/`Rationale:`/`Options:`
markers. The Events API payload for a reaction has no message text, so the
server fetches the message with `conversations.history` and the install's bot
token, then queues it exactly like a marker-bearing mention. Two things to
know:

- **Attribution.** The capture is attributed to the user who *reacted* — the
  actor who took the action. The message's own author and timestamp are kept
  in the decision's evidence, so who wrote the words is not lost.
- **One decision per thread.** Every Slack surface (mention, reaction,
  shortcut) shares the idempotency key `slack://<team>/<channel>/<thread_ts>`.
  A thread that already has a captured decision is not captured a second time;
  the later capture drains as `already_imported`.

Every way a reaction can end without a capture is acknowledged to Slack and
logged by the server (target `hivemind::api::slack`): a message with no
markers, a message posted by an app, a Slack error (`not_in_channel`,
`missing_scope`, a rate limit — the call is never retried), or a Slack timeout.
A `5xx` is deliberately not returned for these, because Slack retries failed
deliveries and disables subscriptions that keep failing.

### Backend support

The Slack app's install/queue storage (`SlackAppStore`) is local-disk JSON
under `--hivemind-dir`. These routes are therefore only wired up when
`hivemind serve` is running the SQLite backend; on the Postgres
(`shared-backend-postgres`) backend they respond `500` naming the gap. A
Postgres-backed install/queue store is a tracked follow-up, not implemented
here.

### Known gaps in this slice

- **A reaction on a thread reply captures nothing.** `conversations.history`
  lists top-level messages only, and a reaction event carries no `thread_ts`
  to look a reply up by, so the reacted-to message cannot be fetched. Use the
  message shortcut on the reply instead — it is given the message directly.
- **A reaction failing is visible only in the server log**, not to the person
  who reacted. There is no reply telling them a message lacked markers or that
  Slack refused the fetch.
- **`/hivemind capture` opens no modal.** A slash command names no message to
  attach a capture to, so it replies with text pointing at the message
  shortcut, the reaction, and `hivemind emit decision.capture`.
- **The shortcut captures the selected message**, not the whole thread: the
  evidence is that one message (author, timestamp, text), keyed to the
  thread it sits in.
- **OAuth state/CSRF** is required to be present and non-empty but is not
  tracked as a single-use nonce (no persistence subsystem for it exists
  yet); callers that need stronger CSRF protection should layer their own
  short-lived state store around it.

## CLI-Driven / Local-First Surface

### App Manifest

Generate a Slack app manifest for the locally hosted request URLs:

```bash
cargo run -- --json slack-app manifest \
  --request-url https://example.ngrok-free.app/slack/interactions \
  --event-url https://example.ngrok-free.app/slack/events \
  --redirect-url https://example.ngrok-free.app/slack/oauth
```

The manifest declares:

- `/hivemind` for `capture`, `query <topic>`, and `show <decision-id>`.
- A message shortcut named `Capture this thread as a decision`.
- A `reaction_added` subscription for the workspace's configured capture emoji.
- Bot scopes for commands, replies, reactions, links, and channel history.

### Workspace Install

Store a workspace installation in the local HiveMind directory:

```bash
cargo run -- --hivemind-dir ./hivemind --json slack-app install \
  --team-id T123 \
  --team-name "Example Workspace" \
  --bot-token "$SLACK_BOT_TOKEN" \
  --signing-secret "$SLACK_SIGNING_SECRET" \
  --hivemind-url http://127.0.0.1:8787 \
  --reaction-emoji hivemind
```

Installation state is written under `./hivemind/slack-app/`. The token file is
created with owner-only permissions on Unix. Slack users default to actor ids of
the form `slack:<workspace>:<user_id>`. Add `--actor-map U123=actor:alice` to
override a Slack user mapping.

### Capture Queue

Slack handlers should acknowledge quickly, then enqueue capture work:

```bash
cargo run -- --hivemind-dir ./hivemind --json slack-app enqueue-capture \
  --team-id T123 \
  --user-id U111 \
  --channel-id C456 \
  --message-ts 1715970800.000100 \
  --permalink https://example.slack.com/archives/C456/p1715970800000100 \
  --surface message_action \
  --title "Use local Slack app capture" \
  --rationale "The reviewed thread records the decision context" \
  --topic-keys slack,integrations \
  --options local-first,hosted-service \
  --chose local-first \
  --thread-text "Thread text or API-fetched excerpt"
```

For reaction-triggered capture, set `--surface reaction --reaction-emoji
hivemind`. The queue drain rejects reaction events whose emoji does not match
the workspace install's configured trigger.

Drain the queue after HiveMind is available:

```bash
cargo run -- --hivemind-dir ./hivemind --json slack-app drain
```

Successful drains remove queue items. Failed items remain in the queue with an
attempt count and last error so they can be retried. Captures are idempotent by
Slack permalink: a retry that already wrote the decision returns the existing
decision id.

### Slack Commands

`/hivemind capture` returns a modal descriptor. The hosting shim opens that modal
with Slack's `views.open` API.

```bash
cargo run -- --hivemind-dir ./hivemind --json slack-app command \
  --team-id T123 --user-id U111 --text capture
```

`/hivemind query <topic>` searches decisions and returns Slack block JSON with
event citations and Slack permalinks when available:

```bash
cargo run -- --hivemind-dir ./hivemind --json slack-app command \
  --team-id T123 --user-id U111 --text "query integrations" --limit 5
```

`/hivemind show <id>` returns the full decision, related option/evidence ids,
and the source citation:

```bash
cargo run -- --hivemind-dir ./hivemind --json slack-app command \
  --team-id T123 --user-id U111 --text "show decision-..."
```

The Slack app code does not summarize threads or infer decisions. Humans review
the modal fields; HiveMind records the resulting decision and preserves the
Slack permalink as evidence.
