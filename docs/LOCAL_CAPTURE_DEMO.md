# Local Capture Demo

This demo proves the local prototype can capture Slack-style human decisions
and direct agent decisions into one HiveMind ledger, then query them through
the same read path.

Run it with:

```bash
cargo test --test local_capture_demo -- --nocapture
```

The test creates a temporary local HiveMind directory, ingests
`tests/fixtures/slack/thread_with_mention.json`, records one Codex decision and
one Claude decision through `emit decision.capture`, and queries
`topic=integrations`, `status=proposed`, `source=slack,agent`.

The printed report intentionally omits generated UUIDs. The assertions still
verify that the raw decision ids returned by the ingest and capture commands are
the same ids returned by query. The report shows the durable provenance contract:

- Slack-style capture records `source=slack`, a Slack thread `source_ref`, and a
  human Slack actor id such as `slack:T123:U111`.
- Direct agent capture records `source=agent`, an agent run `source_ref`, and a
  specific actor id such as `agent:codex:demo-codex`.
- Both paths reach the same ledger and query surfaces without Slack credentials,
  MCP configuration, hosted services, or network access.

The local-first Slack app path in `docs/SLACK_APP.md` replaces the
fixture-backed import with install state, queued Slack-originated captures, and
Slack command responses. The write contract stays the same: every captured
decision keeps explicit `source`, `source_ref`, `actor_id`, and event-origin
provenance.
