# Multi-Tenancy Model

Status: research recommendation for
`hivemind-investigate-multi-tenancy-model-pe6c`.

Question: when HiveMind serves multiple repos, organizations, teams, and users
from the same MCP or API endpoint, how is decision memory scoped, isolated, and
authorized?

## Recommendation

Use an explicit `tenant_id` as HiveMind's top-level isolation boundary. A tenant
is the decision-memory workspace that owns a ledger, graph projection, actor
membership, auth policy, and backup/restore boundary. In most product
deployments this maps to an organization or workspace. It is not a repo, user,
agent session, team, or deployment process.

Repos, teams, Slack workspaces, source documents, and agent sessions are
context within a tenant. They can appear as topic keys, source refs, external
installation mappings, or later narrower access policy, but they do not replace
the tenant boundary.

Every remote write and query is scoped by tenant:

- The service authenticates the caller and resolves one active `tenant_id`.
- The same auth step resolves the `actor_id` taking the action.
- The commands layer receives both values and appends events that carry both.
- The query layer receives an explicit tenant scope and reads only that tenant's
  projected graph.
- Transport layers such as CLI, MCP, Slack, and future HTTP do not enforce
  business rules themselves; they pass the resolved scope into the same internal
  functions.

The existing local SQLite mode remains a single-tenant subset. A local ledger
without `tenant_id` fields is interpreted as one implicit tenant.

## Tenant, Actor, And Principal

HiveMind should keep three identities separate:

| Concept | Meaning | Examples |
| --- | --- | --- |
| `tenant_id` | Decision-memory isolation boundary. | `tenant:acme`, `tenant:local-default` |
| `actor_id` | Human, agent, service, or system actor recorded on events. | `human:alice`, `agent:codex:session-123` |
| `principal_id` | Authenticated credential or login that may act as one or more actors. | OIDC subject, API token id, mTLS client id |

`actor_id` answers "who took this action?" It does not answer "which data may
they access?" A human or agent can belong to multiple tenants, and the same
actor label can appear in more than one tenant. Authorization is the tuple
`(principal_id, tenant_id, actor_id, capability)`.

Projected `Actor` nodes are tenant-scoped. Storage keys should treat
`(tenant_id, actor_id)` as the identity of an actor node inside the graph, even
if the display string is just `actor_id`.

## Event-Level Scoping

Remote event envelopes must include `tenant_id` in addition to the existing
provenance fields:

```rust
pub struct EventEnvelope {
    pub tenant_id: TenantId,
    pub event_id: EventId,
    pub event_uuid: Uuid,
    pub correlation_id: Option<String>,
    pub causation_event_id: Option<EventId>,
    pub event_type: EventType,
    pub actor_id: ActorId,
    pub source: EventSource,
    pub source_ref: Option<String>,
    pub payload: serde_json::Value,
    pub ts: DateTime<Utc>,
}
```

The ledger stays unconditional: tenant validation is deterministic scope and
authorization, not smart behavior. The write path must not search for similar
decisions, deduplicate across tenants, infer tenant from payload text, or repair
missing scope.

Entity ids are unique inside a tenant. A normal decision reference can remain
`decision-123` within one tenant. Cross-tenant or admin references must use an
explicit envelope such as:

```rust
pub struct EntityRef {
    pub tenant_id: TenantId,
    pub entity_id: String,
}
```

Every projected node and edge carries `tenant_id` and `event_origin`. In remote
storage, `event_origin` should be interpreted with `tenant_id`; for portable
audit references, include `event_uuid` in API responses that expose provenance.

## Query Scoping

Every query receives a tenant scope before it reaches graph reads. The default
tenant is the caller's active tenant, resolved from auth or local config. Query
functions must not return unscoped rows and rely on callers to filter them.

Cross-tenant visibility is not part of ordinary reads. If HiveMind later needs
federated decision sharing, it should use one of these explicit shapes:

- an admin-only audit query that returns results partitioned by tenant and
  requires a separate capability;
- an export/import or federation event that creates a tenant-local reference to
  an external decision, preserving the source tenant and event UUID as
  provenance;
- a layer-3 analysis job that reads multiple tenant-scoped result sets and keeps
  citations attached to their original tenant/event refs.

It should not silently merge tenant graphs or let one tenant's query traverse
another tenant's edges.

## Storage Model

Use one service database with tenant-scoped rows for the first shared backend.
This matches the Postgres service direction in `docs/REMOTE_DB.md` and avoids
per-tenant migration, connection-pool, backup, and operational overhead while
HiveMind is still proving the shared service contract.

Recommended remote tables:

```text
events(
  tenant_id text not null,
  event_id bigint not null,
  event_uuid uuid not null,
  correlation_id text,
  causation_event_id bigint,
  event_type text not null,
  actor_id text not null,
  source text not null,
  source_ref text,
  payload jsonb not null,
  ts timestamptz not null,
  primary key (tenant_id, event_id),
  unique (tenant_id, event_uuid)
)

decision_nodes(
  tenant_id text not null,
  decision_id text not null,
  title text not null,
  rationale text not null,
  topic_keys text[] not null,
  event_origin bigint not null,
  primary key (tenant_id, decision_id)
)

relation_edges(
  tenant_id text not null,
  relation text not null,
  from_id text not null,
  to_id text not null,
  event_origin bigint not null
)
```

The same shape applies to actors, evidence, hypotheses, options, blockers, and
notifications. All query indexes must begin with `tenant_id` or otherwise prove
that the tenant predicate is mandatory.

Per-tenant databases are not the default because they make cross-tenant service
operations, migrations, replay parity, and small-customer economics worse. A
dedicated database or dedicated deployment can still be offered later for
customers that need stronger physical isolation; that is a deployment choice,
not a different data model.

## Auth Model

The shared service owns auth and tenancy. Database credentials are never exposed
to agents, CLIs, Slack apps, MCP clients, or UI clients.

Authentication by surface:

| Surface | Auth shape | Tenant resolution |
| --- | --- | --- |
| Local CLI | No service auth; filesystem access to the local ledger. | Implicit local tenant. |
| Remote CLI | Bearer token or login session stored in config. | Token default, `HIVEMIND_TENANT`, or `--tenant` only when the principal has more than one tenant. |
| MCP stdio | Token or server config supplied when the MCP server starts. | Session-bound tenant; tools should not accept arbitrary per-call tenant ids. |
| HTTP/API | OIDC/session for humans; scoped API tokens for agents/services. | Tenant claim or request header validated against principal membership. |
| Slack app | Slack workspace/team install mapped to one tenant. | Install record resolves tenant before queue drain calls commands. |

Authorization checks are deterministic commands-layer inputs. A caller may write
only when the resolved principal can act as the resolved actor in the resolved
tenant. Read queries require a read capability for that tenant. Admin and
federation capabilities are separate from ordinary read/write.

## Interface Sketch

The internal layer should move from per-call actor strings toward a request
context that carries tenant, actor, provenance, and capabilities.

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TenantId(String);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActorId(String);

#[derive(Clone, Debug)]
pub struct CommandContext {
    pub tenant_id: TenantId,
    pub actor_id: ActorId,
    pub provenance: EventProvenance,
    pub capabilities: CapabilitySet,
}

#[derive(Clone, Debug)]
pub struct QueryContext {
    pub tenant_id: TenantId,
    pub actor_id: Option<ActorId>,
    pub capabilities: CapabilitySet,
}
```

Commands should be constructed with `CommandContext`:

```rust
impl<'a, L: EventLedger> Commands<'a, L> {
    pub fn new_with_context(ledger: &'a L, context: CommandContext) -> Self;

    pub fn record_evidence(&self, input: RecordEvidence) -> Result<EvidenceId>;
    pub fn record_hypothesis(&self, input: RecordHypothesis) -> Result<HypothesisId>;
    pub fn record_option(&self, input: RecordOption) -> Result<OptionId>;
    pub fn propose_decision(&self, input: ProposeDecision) -> Result<DecisionId>;
    pub fn accept_decision(&self, input: DecisionIdInput) -> Result<EventId>;
    pub fn reject_decision(&self, input: DecisionIdInput) -> Result<EventId>;
    pub fn supersede_decision(&self, input: SupersedeDecision) -> Result<EventId>;
}
```

The current `Commands::new(...)` and methods that accept `actor_id: &str` can
remain as local compatibility wrappers. They should delegate to the contextual
shape with `TenantId::local_default()` and the supplied actor id.

The ledger trait needs tenant-aware operations:

```rust
pub struct NewEvent {
    pub event_uuid: Uuid,
    pub correlation_id: Option<String>,
    pub causation_event_id: Option<EventId>,
    pub event_type: EventType,
    pub payload: serde_json::Value,
}

pub struct EventRef {
    pub tenant_id: TenantId,
    pub event_id: EventId,
    pub event_uuid: Uuid,
}

pub trait EventLedger {
    fn append(&self, context: &CommandContext, event: NewEvent) -> Result<EventRef>;
    fn read(&self, tenant_id: &TenantId, offset: EventId, limit: usize) -> Result<Vec<Event>>;
    fn replay_from(
        &self,
        tenant_id: &TenantId,
        offset: EventId,
        callback: &mut dyn FnMut(&Event) -> Result<()>,
    ) -> Result<()>;
    fn latest_offset(&self, tenant_id: &TenantId) -> Result<EventId>;
}
```

Query requests should either embed `QueryContext` or receive it as the first
argument:

```rust
pub fn get_decision(
    context: &QueryContext,
    graph: &impl GraphView,
    decision_id: &str,
) -> Result<QueryResponse<Option<DecisionView>>>;

pub fn search_decisions(
    context: &QueryContext,
    graph: &impl GraphView,
    request: SearchDecisionRequest,
) -> Result<QueryResponse<DecisionSearchResults>>;

pub fn get_active_decision_blockers(
    context: &QueryContext,
    graph: &impl GraphView,
    request: ActiveDecisionBlockersRequest,
) -> Result<QueryResponse<DecisionBlockerResults>>;
```

Projection should receive scoped events and write tenant-scoped nodes and edges:

```rust
pub fn project_event(graph: &impl GraphView, event: &Event) -> Result<()>;

pub trait GraphView {
    fn upsert_node(
        &self,
        tenant_id: &TenantId,
        kind: NodeKind,
        id: &str,
        properties: &GraphProperties,
    ) -> Result<()>;

    fn upsert_edge(
        &self,
        tenant_id: &TenantId,
        kind: RelationKind,
        from_id: &str,
        to_id: &str,
        properties: &GraphProperties,
    ) -> Result<()>;

    fn query(
        &self,
        tenant_id: &TenantId,
        cypher: &str,
        params: &GraphParams,
    ) -> Result<Vec<GraphRow>>;
}
```

This keeps tenant enforcement below every transport and above every storage
backend. CLI and MCP only assemble context; commands and queries consume it.

## Surface Implications

Local CLI should keep the current onboarding path:

```bash
hivemind --actor alice emit decision.proposed ...
```

No `--tenant` flag is required in local mode. The local filesystem path already
selects one ledger and therefore one implicit tenant.

Remote CLI can add `--tenant` and `HIVEMIND_TENANT`, but only as tenant
selection among memberships already present in the caller's credential. It must
not be a free-form override that lets a user ask for another tenant's data.

MCP should bind tenant at server startup or auth handshake. The current tool
arguments should not grow a required `tenant_id` field for every call, because
that would put authorization-sensitive scope selection in a model-generated tool
payload. The MCP server should resolve `CommandContext` and `QueryContext`, then
call the same internal functions as CLI and HTTP.

HTTP should expose tenant in a conventional authenticated shape, such as a token
claim plus optional `X-HiveMind-Tenant` header when a principal has multiple
memberships. The service validates the selected tenant before calling commands
or queries.

## Migration From Local Single-Tenant

Existing local SQLite ledgers remain valid. They are read as:

```rust
TenantId::local_default()
```

No local event JSON needs to be rewritten before the current CLI can read it.
When a user or organization migrates to a remote tenant:

1. Create the remote tenant and actor memberships.
2. Replay local events in ascending local `event_id`.
3. For each event, append a remote event under the chosen `tenant_id`, preserving
   `event_uuid`, `correlation_id`, `causation_event_id`, `actor_id`, `source`,
   `source_ref`, payload, and original timestamp when policy permits.
4. Record an import mapping from local offset to remote `EventRef`.
5. Rebuild or transactionally update the remote projection.
6. Compare deterministic query results between the local ledger and remote
   tenant for the migrated range.

Remote `event_id` values may differ from local offsets. Audit UI should show the
remote event ref and may also show the imported local offset as source
provenance. The source of truth after migration is the remote tenant ledger.

## Principles Cross-Check

- **The ledger is unconditional:** tenant resolution is required scope, not
  similarity, ranking, or model behavior.
- **Provenance is mandatory:** events carry both `tenant_id` and `actor_id`, so
  "who did this?" and "which decision memory owns it?" are both auditable.
- **Layer boundaries stay enforced:** CLI, MCP, Slack, and HTTP resolve context;
  commands validate/write; queries read scoped projections; layer 3 receives
  bounded tenant-scoped query results.
- **Disagreement survives:** tenant scoping must not introduce uniqueness
  constraints that collapse different actors' accept/reject or supersession
  events.
- **No silent staleness or truncation:** query responses keep the existing
  `truncated` contract and compute status inside the active tenant's explicit
  graph.
