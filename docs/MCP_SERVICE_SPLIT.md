# MCP / Service-API Split

Status: shipped. MCP is exposed natively from the Rust core over two
transports (stdio for local self-host, HTTP for the shared service); there is
no separate MCP process or implementation language. Original design (a
TypeScript gateway) captured 2026-06-13; that direction was **declined
2026-06-20** in favor of the native transport described below, and the
`clients/mcp-gateway/` code this doc used to describe is removed as of this
revision.

Question answered: when HiveMind goes multi-tenant, where does the MCP server
live, in what language, and how does it relate to the Rust core?

## Governing principle: strong types as the AI's correctness oracle

HiveMind is written by AI under human direction. The compiler is therefore not
just a safety net — it is the model's feedback loop. The stricter the type
system, the more AI mistakes (hallucinated methods, wrong shapes, unhandled
variants) it rejects before they ship. Rank languages by
"compiler-as-AI-oracle" strength, roughly: **Rust > C# ≈ strict-TS > Go ≫
Python.** This is why the core is Rust, and why any new surface should default
to staying in Rust unless something concrete about the surface requires
otherwise.

## Decision: keep MCP in Rust, including the networked/multi-tenant form

The original 2026-06-13 proposal argued that multi-tenant MCP — a networked,
authenticated, tenant-scoped service — was the one corner where TypeScript
genuinely beat Rust, because the MCP SDK is TS-first and the streamable-HTTP /
session / auth plumbing was more mature there. A thin TS stdio gateway
(`clients/mcp-gateway/`) shipped 2026-06-16 on that basis, forwarding bearer
tokens to the Rust HTTP API and exposing 4 read-only tools.

That argument didn't survive contact with the auth work. `axum` (already the
REST transport) handles the streamable-HTTP framing and session-header
plumbing for MCP with no more ceremony than a REST route, and the same
`RequestContext`-resolution used by `/v1/*` (see [`AUTH_MODEL.md`](AUTH_MODEL.md))
covers MCP unchanged. 2026-06-20 added `POST /mcp` directly in the Rust
service — MCP 2025-03-26 Streamable HTTP, JSON-RPC 2.0, session id issued on
`initialize` — and the TS-extraction direction was declined the same day:
there was no longer a capability gap to extract *for*, only a second language,
a second deploy artifact, and a second place the tool surface could drift from
`tool_definitions()`. `clients/mcp-gateway/` sat unreferenced by any shipped
config, script, or CI job from that point on and is deleted in this revision;
`scripts/install-mcp.sh`, which existed only to build and wire up that
gateway, is deleted with it.

This keeps the property the original doc's "hard rule" section was trying to
protect — one engine, no drift-prone second copy of the query/auth logic — by
construction rather than by convention: MCP-over-HTTP and MCP-over-stdio both
call `commands`/`queries` in-process, the same functions the CLI and REST call,
per [`ARCHITECTURE.md`](ARCHITECTURE.md)'s Surface Uniformity commitment.

## What shipped

- `src/mcp.rs` — `tool_definitions()`, the single source of truth for the tool
  list (schema, description, read/write/layer-3 classification) shared by
  both transports, plus the stdio JSON-RPC serve loop (`serve_stdio`) used for
  local self-host (`hivemind mcp`).
- `src/api/mcp_http.rs` — `POST /mcp`, the Streamable HTTP transport for the
  shared/hosted service. Routed at `/mcp` in `src/api.rs`. Auth goes through
  the same `extract_ctx` bearer/session resolution as `/v1/*`
  ([`REMOTE_DB.md`](REMOTE_DB.md)); `WWW-Authenticate` on 401 points MCP
  clients at the OAuth protected-resource metadata endpoint.
- Tool count: 21 (5 write, 16 read, per the generated reference's own split)
  as of this revision — verify against `tool_definitions()` rather than
  trusting this number as it drifts. The generated reference is kept honest by
  `cargo run --bin generate-reference -- --check` (mandatory quality gate; see
  [`QUALITY_GATES.md`](QUALITY_GATES.md)), which fails the build if
  `website/src/content/docs/reference/mcp-tools.md` stops matching
  `tool_definitions()`.
- Capture-path framing: [`AGENT_DECISION_CAPTURE.md`](AGENT_DECISION_CAPTURE.md)
  covers where MCP sits among the capture paths (CLI, hooks, sidecar, MCP).
- User-facing setup: [MCP Setup](../website/src/content/docs/guides/mcp-setup.md)
  documents both the managed remote endpoint (browser OAuth, no local
  install) and the local stdio server for self-host; the tool table there is
  hand-maintained prose, not generated, so treat
  [MCP Tools reference](../website/src/content/docs/reference/mcp-tools.md)
  (generated, gated) as canonical for the exact tool list.
- Multi-tenant RLS — Postgres Row-Level Security keyed on `tenant_id`,
  enforced at the database layer, applies identically regardless of which
  transport a request arrived on. See [`MULTI_TENANCY.md`](MULTI_TENANCY.md).

## See also

- [`MULTI_TENANCY.md`](MULTI_TENANCY.md) — tenant as the top-level isolation boundary.
- [`AUTH_MODEL.md`](AUTH_MODEL.md) — bearer tokens, OIDC sessions, tenant/capability scoping.
- [`REMOTE_DB.md`](REMOTE_DB.md) — Postgres-backed HiveMind service and API boundary.
- [`ARCHITECTURE.md`](ARCHITECTURE.md) — the three-layer boundary and Surface Uniformity commitment every transport must respect.
- [`AGENT_DECISION_CAPTURE.md`](AGENT_DECISION_CAPTURE.md) — how MCP fits among the capture paths.
