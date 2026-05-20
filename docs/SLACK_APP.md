# Slack App

HiveMind includes a local-first Slack app surface for human decision capture and
read-only decision queries. The app keeps Slack as an integration layer: Slack
requests are queued, drained into the normal HiveMind command layer, and query
responses are rendered from deterministic read APIs.

## App Manifest

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

## Workspace Install

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

## Capture Queue

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

## Slack Commands

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
