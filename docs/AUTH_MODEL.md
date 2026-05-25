# Auth Model

Status: accepted recommendation for
`hivemind-m2-shared-backend-lives-uuq9.5`.

Question: how does a shared HiveMind service authenticate a caller and resolve
that caller to a tenant and actor?

## Decision

Use service-owned authentication that resolves every remote request into an
explicit request context:

```rust
pub struct RequestContext {
    pub tenant_id: TenantId,
    pub actor_id: ActorId,
    pub principal_id: PrincipalId,
    pub credential_id: CredentialId,
    pub capabilities: CapabilitySet,
    pub signature: Option<VerifiedSignature>,
}
```

The primary machine credential is an opaque scoped bearer token. Human product
surfaces use OIDC-backed login sessions. Ed25519 signing is a mandatory write
integrity layer for multi-organization service deployments, not a replacement
for authorization.

The service, not the caller, owns tenant resolution. A caller may select among
tenants already present in its credential or session, but it may not provide a
free-form tenant id and expect the commands or query layer to trust it.

## Credential Types

| Credential | Use | Decision |
| --- | --- | --- |
| Opaque bearer token | Agents, services, remote CLI, MCP server processes. | Adopt as the default machine credential. Tokens are random secrets with display prefixes such as `hm_sk_live_...`, stored server-side only as hashes, and logged only by token id. |
| OIDC login/session | Human web UI and future interactive product flows. | Adopt for humans. OIDC resolves a stable `principal_id`; HiveMind still maps that principal to tenant, actor, and capabilities before calling commands or queries. |
| Ed25519 signing key | Write integrity and audit hardening. | Require for remote writes in multi-organization deployments. Signing proves the accepted request or event envelope was bound to an authorized credential; it does not by itself choose tenant or capability. |
| mTLS client identity | Deployment hardening between trusted services. | Allow as an infrastructure control, but do not make it the product auth model. It can resolve a `principal_id` in private deployments. |
| Short-lived JWT | Cached session or edge token minted by the HiveMind service. | Allow as an internal optimization after OIDC or token validation. Do not use self-issued JWTs from clients as the canonical auth secret. |

This is deliberately a hybrid model because humans and agents have different
credential ergonomics. The invariant is that every accepted request reaches the
core layers as the same `RequestContext`.

## Token Record

Bearer tokens are not actors. A token authenticates a principal that may be
allowed to act as one or more actors in one or more tenants.

```rust
pub struct TokenRecord {
    pub token_id: CredentialId,
    pub principal_id: PrincipalId,
    pub default_tenant_id: Option<TenantId>,
    pub allowed_tenant_ids: Vec<TenantId>,
    pub allowed_actor_ids: ActorSelectorSet,
    pub capabilities: CapabilitySet,
    pub signing_key_ids: Vec<SigningKeyId>,
    pub expires_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}
```

Token lookup uses the secret only to find a hashed record. After validation,
all logs, audit metadata, and error messages use `token_id`, never the secret.

Capabilities are tenant-scoped. A token with `write:decision` in
`tenant:acme` has no implied access to `tenant:globex`.

## Tenant Resolution

The auth layer resolves exactly one active tenant before commands or queries
run:

1. Validate the credential or login session.
2. Load the principal's tenant memberships and capabilities.
3. Select a tenant from, in order:
   - a token default tenant;
   - an OIDC session active tenant;
   - `HIVEMIND_TENANT`, `--tenant`, or `X-HiveMind-Tenant`, when the principal
     has more than one membership.
4. Reject the request if the selected tenant is absent or not in the
   principal's membership set.
5. Pass the resolved `tenant_id` as part of `RequestContext`.

Tenant selection is a transport concern. It is not a payload field on decision,
evidence, hypothesis, or relation events. The write path appends the resolved
tenant into the event envelope; query functions receive the resolved tenant
before graph reads.

## Actor Resolution

`actor_id` remains the provenance identity recorded on events. Existing agent
session conventions stay valid:

- `agent:codex:<session>`
- `agent:claude:<session>`
- `human:<stable-human-label>`
- `service:<service-name>`

The authenticated principal must be allowed to act as the resolved actor in the
resolved tenant. For example, a Codex MCP token may allow
`agent:codex:*` for one tenant, while a narrow CI token may allow only
`service:ci-release-gate`.

Remote MCP should bind tenant and default actor when the server starts or when
the service auth handshake completes. Individual tool calls should not accept
arbitrary tenant ids. If a remote tool accepts `actor_id` for local compatibility,
the service still validates it against `allowed_actor_ids`.

## Ed25519 Signing

Ed25519 is the audit integrity layer required by the shared multi-organization
direction. It answers a different question from bearer tokens:

- bearer token or OIDC: "is this principal authorized for this tenant, actor,
  and capability?"
- Ed25519 signature: "was this accepted write bound to the authorized key and
  canonical request body?"

For remote writes, the canonical signature input should include:

- HTTP method or transport operation name;
- path or tool name;
- tenant id;
- actor id;
- credential id;
- timestamp and nonce;
- SHA-256 of the canonical JSON body.

The service verifies the signature before appending the event. The event
envelope records non-secret signature metadata:

```rust
pub struct EventAuth {
    pub principal_id: PrincipalId,
    pub credential_id: CredentialId,
    pub signing_key_id: Option<SigningKeyId>,
    pub signature_verified: bool,
}
```

Local single-tenant CLI ledgers may continue without signatures. A remote
multi-organization service must set tenant policy so accepted writes either
carry a verified actor/client signature or are signed by a trusted service key
inside a human session boundary. Unsigned remote writes are a development mode,
not the production target.

## Surface Sketches

Remote CLI:

```bash
hivemind \
  --remote https://hivemind.example.com \
  --token-env HIVEMIND_TOKEN \
  --tenant tenant:acme \
  --actor agent:codex:gc-47745 \
  emit decision.proposed \
  --title "Adopt scoped API tokens for agents" \
  --rationale "The service must resolve tenant, actor, and capabilities before append"
```

MCP server startup:

```bash
hivemind mcp \
  --remote https://hivemind.example.com \
  --token-env HIVEMIND_TOKEN \
  --tenant tenant:acme \
  --agent-tool codex \
  --session-id gc-47745
```

HTTP write:

```http
POST /v1/decisions HTTP/1.1
Authorization: Bearer hm_sk_live_<secret>
X-HiveMind-Tenant: tenant:acme
X-HiveMind-Actor: agent:codex:gc-47745
X-HiveMind-Signature-Key: key_2026_05_codex
X-HiveMind-Signature-Timestamp: 2026-05-25T07:00:00Z
X-HiveMind-Signature-Nonce: 7f4a0b2c
X-HiveMind-Signature: ed25519=<base64-signature>
Content-Type: application/json
```

The HTTP handler verifies auth and signature, builds `RequestContext`, then
calls the same commands layer as CLI and MCP.

## Commands And Queries

The core layers should receive context, not raw credentials:

```rust
pub struct CommandContext {
    pub tenant_id: TenantId,
    pub actor_id: ActorId,
    pub auth: EventAuth,
    pub provenance: EventProvenance,
    pub capabilities: CapabilitySet,
}

pub struct QueryContext {
    pub tenant_id: TenantId,
    pub actor_id: Option<ActorId>,
    pub principal_id: PrincipalId,
    pub capabilities: CapabilitySet,
}

impl<'a, L: EventLedger> Commands<'a, L> {
    pub fn new_with_context(ledger: &'a L, context: CommandContext) -> Self;
}
```

The commands layer may reject a context that lacks the required capability, but
it must not parse bearer tokens, call an identity provider, rank tenants, infer
actor identity from text, or consult layer-3 behavior. Query code receives a
`QueryContext` and reads only that tenant's projection.

Local compatibility wrappers can keep the current API:

```rust
hivemind --actor human:alice emit decision.proposed ...
```

Those wrappers build `TenantId::local_default()` and local provenance, then
delegate to the contextual shape.

## Rejected Alternatives

Bearer-only auth is too weak for the multi-organization audit target. It
handles revocation and ergonomics well, but it does not satisfy the signing
commitment by itself.

Signing-key-only auth is too narrow. A valid signature proves key possession,
not tenant membership, capability, token expiry, or human login status.

OIDC-only auth is wrong for agents and MCP because many autonomous callers do
not have a browser session. OIDC remains the right human login layer.

mTLS-only auth couples product identity to deployment topology and is awkward
for hosted CLI, MCP, and third-party integrations.

Client-issued JWT-only auth makes revocation, tenant membership changes, and
key rotation harder than opaque server-owned tokens. JWTs are acceptable only
when minted by the service after canonical auth.

## Principles Cross-Check

- **Provenance is mandatory:** every write records `actor_id`, source, session,
  resolved tenant, credential id, and signature metadata when applicable.
- **The ledger is unconditional:** auth resolves deterministic context before
  append; no similarity, ranking, model call, or deduplication participates.
- **Layer boundaries stay enforced:** transports authenticate, commands
  validate capabilities and append, queries read scoped projections, and
  layer 3 receives only bounded query results.
- **Disagreement survives:** auth constrains who may write, but it does not add
  uniqueness rules that collapse contested decisions or concurrent
  supersessions.
- **No silent staleness:** tenant-scoped query responses still surface refuted
  hypotheses and superseded decisions inside the resolved tenant.
