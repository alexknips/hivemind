# HTTP API

Status: implemented for `hivemind-m2-shared-backend-lives-uuq9.6`.

HiveMind exposes HTTP as JSON-RPC 2.0 over `POST /v1/rpc`. JSON-RPC was chosen
over REST for this slice because the existing CLI and MCP surfaces are already
operation-oriented. Keeping method names close to the CLI and MCP names lets the
HTTP transport stay a thin wrapper over `commands` and `queries` instead of
inventing resource-specific behavior.

`hivemind serve --port 8080` starts the API. The default bind host is
`127.0.0.1`; pass `--host` to bind another interface.

```bash
hivemind --hivemind-dir ./hivemind/ serve --port 8080
```

## Request Context

Every RPC request carries tenant and actor context in headers:

```http
X-HiveMind-Tenant: tenant:acme
X-HiveMind-Actor: agent:codex:session-123
X-HiveMind-Source-Ref: api-client-optional
```

`X-HiveMind-Tenant` resolves the `TenantId` passed into the query or command
context. `X-HiveMind-Actor` is the actor recorded on write events. If
`X-HiveMind-Source-Ref` is absent, writes use the actor id as the API source
reference. Per-request payload `actor_id` is accepted only when it matches the
header actor, which keeps local MCP-compatible payloads usable without letting
payload data override the transport context.

This local HTTP slice does not implement bearer-token lookup or Ed25519
verification yet. Those remain service-auth responsibilities described in
[`AUTH_MODEL.md`](AUTH_MODEL.md); the command and query layers still receive
resolved context, not credentials.

## Methods

Write methods:

- `emit.decision.capture` and `emit.decision.proposed`
- `emit.decision.accepted`
- `emit.decision.rejected`
- `emit.decision.superseded`
- `emit.evidence.recorded`
- `emit.hypothesis.recorded`
- `emit.option.recorded`
- `emit.relation.added`
- `emit.relation.attach_evidence`
- `disagree`
- `supersede`

MCP-compatible aliases are also accepted for the existing agent surface:
`capture_decision`, `capture_evidence`, `capture_hypothesis`,
`disagree_decision`, and `supersede_decision`.

Read methods:

- `query.get_decision`
- `query.get_relevant_decisions`
- `query.get_supersession_chain`
- `query.get_decision_neighborhood`
- `query.search` and `query.search_decisions`
- `query.get_active_decision_blockers`
- `query.get_blocker_notification_candidates`
- `query.recent`
- `query.get_recent_activity`
- `query.get_decisions_changed_since`
- `query.get_decisions_added_since`
- `query.export_read_only_summary`
- `dump`

MCP-compatible read aliases are accepted for `get_decision`,
`get_relevant_decisions`, `get_supersession_chain`, `search_decisions`, and
`dump_graph`.

## Example

```bash
curl -sS http://127.0.0.1:8080/v1/rpc \
  -H 'Content-Type: application/json' \
  -H 'X-HiveMind-Tenant: tenant:acme' \
  -H 'X-HiveMind-Actor: agent:codex:session-123' \
  -d '{
    "jsonrpc": "2.0",
    "id": 1,
    "method": "emit.decision.capture",
    "params": {
      "title": "Expose HiveMind over HTTP JSON-RPC",
      "rationale": "A third transport should reuse the same command layer",
      "topic_keys": ["api"],
      "options": ["json-rpc", "rest"],
      "chosen_option_label": "json-rpc"
    }
  }'
```

Successful responses are JSON-RPC envelopes whose `result` is the same JSON
shape used by the corresponding CLI or MCP operation. Query responses preserve
the standard `result_count`, `truncated`, `latency_ms`, and `data` fields.
Errors are JSON-RPC error envelopes; missing tenant or actor headers return
HTTP 401 with an error code of `-32001`.
