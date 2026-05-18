# HiveMind Integration Paths

Purpose: choose the smallest reliable capture paths for a shared remote HiveMind
service. All integrations call the hosted commands/query API; none write local
files or bypass the commands module.

## Evaluation Matrix

| Path | Reliability | Latency/perf | Cost | Ops overhead | Install fit | Security/privacy | UX fit |
|---|---|---:|---|---|---|---|---|
| Slack slash command + message shortcut | High when user-initiated; survives app restarts if requests are acked and retried | Fast if endpoint acks within Slack's 3s interaction window and does work async | Low hosting + Slack app maintenance; no model call required | Moderate: Slack app, OAuth install, signing verification, queue, logs | Good for team Slack; one workspace can install from manifest | Start with `commands`; add `chat:write` only for channel confirmations; avoid history scopes in phase 1 | Best human capture: explicit intent, modal review, clear provenance |
| Slack marked-thread event listener | Medium for suggestions; not reliable enough for critical capture | Near-real-time but retries/backoff can delay work | Higher if summarizing threads with a model | Higher: event subscriptions, channel membership, retry handling, dead letters, scope drift | Harder: bot must be invited/scoped to channels | History scopes expose more workspace data; private channels need broader permissions | Good for drafts from `:hivemind:`/mention, poor as the only capture path |
| MCP server | High when the agent client consistently invokes tools; variable across clients | Low overhead for stdio; HTTP adds auth round trips | Low server cost; client install/support cost | Moderate: tool schemas, client configs, auth for hosted HTTP | Good in MCP-aware tools; uneven elsewhere | HTTP MCP needs OAuth/audience checks; stdio should read credentials from env | Best portable agent-tool surface for propose/query once installed |
| Skill/instruction pack | Medium; depends on model following instructions | No runtime overhead until it calls CLI/API | Very low | Low, but must be kept current per agent product | Easy for Claude/Codex/Gemini-style agents | Secrets must not live in instructions; use env/config | Good onboarding and prompts, not a reliable transport by itself |
| Git/tool hook or shell wrapper | Medium to low; easy to skip, bypass, or break in non-git flows | Usually fast; can slow commits/builds if synchronous | Low | Medium: per-repo install, failure policy, cross-shell support | Good for one developer repo; poor for hosted customers/non-code work | Local tokens and logs need care; repo hooks are not tenant boundaries | Useful safety net for coding workflows, not human Slack capture |
| Direct CLI/API from automation | Highest when called by deterministic jobs/agents | Fast; bounded by API/database write latency | Lowest moving-parts cost | Low: API token, retry/idempotency, logs | Good for agents, CI, scripts; less friendly for non-devs | Clear actor tokens and audit trail; easiest to scope per tenant | Best critical agent capture path |

## Phase-1 Recommendation

Build two explicit capture channels against the shared remote service:

1. Human Slack capture: a Slack app with one slash command (`/hivemind`) and one
   message shortcut (`Capture decision`). Both open/submit a small modal:
   title, rationale, status intent (`proposed` or `accepted`), topic keys,
   optional option/evidence links, and source permalink. The Slack endpoint
   verifies signatures, immediately acknowledges, enqueues the command, and
   posts an ephemeral success/error response. First install should require only
   the `commands` scope; add `chat:write` only if persistent channel
   confirmations are needed.

2. Agent capture: expose direct HTTPS command/query endpoints and a tiny CLI
   wrapper first. Add an MCP server as a thin adapter over those same endpoints
   for tools that support MCP. The MCP server should expose boring tools such as
   `propose_decision`, `accept_decision`, `reject_decision`,
   `supersede_decision`, `record_evidence`, and `get_decision`; it must not
   summarize, deduplicate, rank, or infer.

Do not rely on passive Slack event listeners, skills/instruction packs, or git
hooks for critical decision capture. They are useful reminders or draft
generators, but their execution is inconsistent enough that the canonical path
must be an explicit Slack action or a deterministic API/CLI/MCP tool call.

## Architecture Notes

- All adapters authenticate an `Actor` and call layer-1 commands or layer-2
  queries through the hosted API.
- Slack message/thread text is evidence, not a decision, until a human submits
  the reviewed modal.
- Passive Slack listeners may create `decision.proposed` drafts only after an
  explicit marker such as a message shortcut, app mention, or reaction. Any
  LLM-based thread summarization belongs in layer 3 and must cite source
  messages as evidence.
- API writes require idempotency keys mapped to `event_uuid`; Slack retry ids
  and agent run ids should become correlation ids.
- Query responses remain bounded and honest: propagate `truncated`, status, and
  stale/refuted assumptions instead of hiding partial context.
- Tenant isolation belongs at the hosted service boundary: Slack team/workspace,
  actor token, and organization id must map to a single HiveMind tenant.

## Follow-Up Beads

- Implement hosted command/query API backed by the existing commands and query
  modules.
- Prototype Slack app manifest plus `/hivemind` command and `Capture decision`
  message shortcut.
- Add Slack interaction handler with signature verification, 3-second ack,
  async enqueue, and idempotent retries.
- Build agent CLI direct capture against the hosted API with env-based actor
  token config.
- Build MCP stdio server as a thin adapter over the same API and document
  client config for Codex/Claude/Gemini-style tools.
- Add optional Slack marked-thread draft listener after explicit capture works.

## References

- Slack slash commands and message shortcuts are app interactivity entry points:
  https://docs.slack.dev/interactivity/implementing-slash-commands/
- Slack app manifests define slash commands, shortcuts, and interactivity URLs:
  https://docs.slack.dev/reference/app-manifest/
- Slack Events API is useful but scope- and retry-dependent:
  https://docs.slack.dev/apis/events-api/
- MCP exposes JSON-RPC tools/resources/prompts with optional HTTP authorization:
  https://modelcontextprotocol.io/specification/2025-11-25/basic
