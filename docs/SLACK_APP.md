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

Set the manifest's Request URL / Event Subscriptions URL / OAuth Redirect URL
to `https://<your-host>/v1/slack/commands` (interactivity — see below),
`https://<your-host>/v1/slack/events`, and
`https://<your-host>/v1/slack/oauth/callback` respectively:

```bash
cargo run -- --json slack-app manifest \
  --request-url https://your-host/v1/slack/commands \
  --event-url https://your-host/v1/slack/events \
  --redirect-url https://your-host/v1/slack/oauth/callback
```

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

`POST /v1/slack/events` never writes to the ledger inline — it enqueues onto
the same `SlackAppStore` capture queue the CLI's `slack-app enqueue-capture`
uses, and returns within Slack's 3-second ack window. A background task
(started once, from `hivemind serve`) polls the queue and drains it into
each capture's own tenant ledger every 15 seconds.

`app_mention` and `message.channels` events are auto-captured only when the
message text carries the same `Decision:`/`Rationale:`/`Options:` markers
`hivemind ingest slack-thread` already parses (see "Capture Queue" below) —
no LLM inference happens on the ingest path, per AGENTS.md's three-layer
separation. A message without those markers is acknowledged and ignored.

### Backend support

The Slack app's install/queue storage (`SlackAppStore`) is local-disk JSON
under `--hivemind-dir`. These routes are therefore only wired up when
`hivemind serve` is running the SQLite backend; on the Postgres
(`shared-backend-postgres`) backend they respond `500` naming the gap. A
Postgres-backed install/queue store is a tracked follow-up, not implemented
here.

### Known gaps in this slice

- **`reaction_added` does not complete a capture.** The signature is
  verified and a matching reaction (against the install's configured
  `reaction_emoji`) is logged, but the Events API reaction payload carries
  no message text — completing it needs an outbound Slack Web API call
  (`conversations.history` or similar) this slice does not add.
- **`POST /v1/slack/interactivity` is not implemented.** Block actions and
  view submissions — the "Capture this thread as a decision" message
  shortcut and its modal — need a separate outbound `views.open` call and
  are out of scope here; see the follow-up bead this slice's bead names.
  `/hivemind capture` over the commands route above returns a plain text
  reply pointing at the CLI instead of silently claiming a modal will open.
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
