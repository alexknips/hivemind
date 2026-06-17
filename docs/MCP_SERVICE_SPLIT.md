# MCP / Service-API Split (TypeScript + Effect)

Status: shipped in M2. The TS MCP gateway, HTTP service API, and multi-tenant
RLS isolation are all in master. Original design discussion captured on
2026-06-13.

Question answered: when HiveMind goes multi-tenant, where does the MCP server
live, in what language, and how does it relate to the Rust core?

## Governing principle: strong types as the AI's correctness oracle

HiveMind is written by AI under human direction. The compiler is therefore not
just a safety net — it is the model's feedback loop. The stricter the type
system, the more AI mistakes (hallucinated methods, wrong shapes, unhandled
variants) it rejects before they ship. Verbosity that costs a human is largely
pre-paid by the AI, and the strictness is pure upside.

Rank languages by "compiler-as-AI-oracle" strength, roughly:
**Rust > C# ≈ strict-TS > Go ≫ Python.** This is why the core is Rust (single
static binary, correctness-critical schema-driven event-sourced ledger, clean
C++ FFI for Kùzu via `cxx`, and intelligence delegated to LLM APIs rather than
an in-process ML stack), why we lean against Go (statically typed but
*inexpressive* — no sum types / exhaustiveness / nil-safety — which bites the
`EventType` model), and why we reject Python for new surfaces (its hints are
advisory, not blocking). Any new surface must preserve the type-guardrail
property.

## Decision: extract MCP to TypeScript (Effect) at the multi-tenancy step

While MCP was local JSON-RPC over stdio against an in-process `EventLedger`, it
stayed in Rust. At multi-tenancy, MCP changes kind: it becomes a **networked,
authenticated, tenant-scoped service** (opaque bearer tokens `hm_sk_live_…`,
remote MCP server processes, tenant-scoped capabilities). That is the one corner
where TypeScript genuinely beats Rust: the MCP SDK is TS-first, and the
streamable-HTTP / session / auth plumbing is more mature there.

The extracted server is in `clients/mcp-gateway/`. It uses Node.js with the
`@modelcontextprotocol/sdk` — not Effect TS as originally proposed, because the
read-only gateway surface turned out to be simple enough that Effect's
structured concurrency and typed errors would have been overkill. The
type-guardrail property is preserved through strict TypeScript (`tsconfig.json`
has `strict: true`) and a typed `HiveMindClient` wrapper in `src/client.ts`.

## The hard rule: the TS MCP is a thin protocol adapter, nothing more

The Rust HiveMind service owns everything that matters; the TS gateway owns only
the protocol edge.

Rust (the service) owns:
- Auth enforcement — bearer-token validation, tenant and capability scoping.
- All queries, the event ledger, and projections. A single, server-side
  enforcement point.

TS MCP owns only:
- The MCP protocol (`initialize`, `tools/list`, `tools/call`,
  `notifications/initialized`).
- Transport (stdio).
- Forwarding the caller's bearer token to the service unmodified.
- Mapping tool calls → HiveMind service API calls (`/v1/decisions/*`), and
  returning results.

The TS MCP must **not**:
- Talk to Postgres directly. That bypasses capability enforcement and forks the
  Rust query layer into a second, drift-prone engine.
- Embed any domain or query logic.
- Mint or validate auth itself. It forwards tokens; Rust validates them.

## What shipped

- `clients/mcp-gateway/` — TypeScript stdio MCP gateway.
  - `src/client.ts` — typed `HiveMindClient` wrapping all `/v1` read calls.
  - `src/index.ts` — MCP server, tool definitions, stdio transport.
  - Tools: `get_decision`, `get_relevant_decisions`, `search_decisions`,
    `get_supersession_chain`.
  - Auth: `HIVEMIND_API_KEY` forwarded as `Authorization: Bearer <token>` on
    every request. Tenant scope enforced server-side (RLS).

- `src/api.rs` — HTTP REST API (`/v1/*`), the service the gateway calls. See
  [`REMOTE_DB.md`](REMOTE_DB.md) for the full API boundary description.

- Multi-tenant RLS — Postgres Row-Level Security keyed on `tenant_id`,
  enforced at the database layer. See [`MULTI_TENANCY.md`](MULTI_TENANCY.md).

## Configuration

```
HIVEMIND_URL      Base URL of the HiveMind HTTP service (no trailing slash)
HIVEMIND_API_KEY  Bearer token (hm_sk_live_...)
```

Run:

```bash
cd clients/mcp-gateway
npm install
node dist/index.js
```

Or wire it into an MCP client via `mcp.json`:

```json
{
  "mcpServers": {
    "hivemind": {
      "command": "node",
      "args": ["/path/to/clients/mcp-gateway/dist/index.js"],
      "env": {
        "HIVEMIND_URL": "https://your-hivemind-server",
        "HIVEMIND_API_KEY": "hm_sk_live_..."
      }
    }
  }
}
```

## The contract is single-sourced from `schemas/v0`

The boundary between the Rust service and the TS gateway must not drift between
languages. The `schemas/v0/*.json` files are the contract's source of truth.
The TS client in `src/client.ts` mirrors those shapes; when a schema changes,
update the TS types to match.

## See also

- [`MULTI_TENANCY.md`](MULTI_TENANCY.md) — tenant as the top-level isolation boundary.
- [`AUTH_MODEL.md`](AUTH_MODEL.md) — bearer tokens, tenant/capability scoping, remote MCP.
- [`REMOTE_DB.md`](REMOTE_DB.md) — Postgres-backed HiveMind service and API boundary.
- [`ARCHITECTURE.md`](ARCHITECTURE.md) — the three-layer boundary the gateway must respect.
