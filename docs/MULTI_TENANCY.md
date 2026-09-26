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
the tenant boundary. The one grouping HiveMind records for them is the
decision's project ([Projects Inside A Tenant](#projects-inside-a-tenant)): a
named home for decisions inside one tenant. A project organizes decisions; it
does not isolate them or gate who may read them.

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

The accepted credential and token decision is recorded in
[`AUTH_MODEL.md`](AUTH_MODEL.md). This section summarizes how that auth model
feeds tenant resolution.

The shared service owns auth and tenancy. Database credentials are never exposed
to agents, CLIs, Slack apps, MCP clients, or UI clients. Authentication resolves
a principal; authorization resolves the tuple
`(principal_id, tenant_id, actor_id, capability)` before any command or query
runs.

Authentication by surface:

| Surface | Auth shape | Tenant resolution |
| --- | --- | --- |
| Local CLI | No service auth; filesystem access to the local ledger. | Implicit local tenant. |
| Remote CLI | Scoped opaque bearer token for agents/services or OIDC-backed login session for humans. | Token/session default, `HIVEMIND_TENANT`, or `--tenant` only when the principal has more than one tenant. |
| MCP stdio | Scoped bearer token and server config supplied when the MCP server starts. | Session-bound tenant; tools should not accept arbitrary per-call tenant ids. |
| HTTP/API | OIDC/session for humans; scoped opaque bearer tokens for agents/services; Ed25519 signatures for remote multi-org writes. | Tenant claim or `X-HiveMind-Tenant` header validated against principal membership. |
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

## Projects Inside A Tenant

A tenant isolates; a project organizes. Inside one tenant, every decision belongs
to exactly one **project**: a named home for decisions, such as `billing` or
`platform`. A decision is never filed under two projects. The rules below are
the ones the write and query layers enforce today; the last subsection lists
what is not built.

- A project is not an access boundary. There is no membership and no permission
  on a project. Everyone who can read the tenant can read every project, and
  "who works on Billing" is answered from whose decisions are in it.
- A project is not a topic. A topic says what a decision is about (`pricing`), a
  project says where it belongs (`billing`). A topic cuts across projects and
  both remain filters.
- Nothing crosses a tenant. Projects, links, and moves live in one tenant's
  ledger, and a link cannot point at another tenant's project.
- HiveMind never infers a project. The write layer checks that a stated handle
  is registered and records it with how the caller determined it; working it out
  is the client's job (see [Where the project comes from](#where-the-project-comes-from)).

### Three kinds of address

| Address | Made | A wrong one |
| --- | --- | --- |
| Tenant (`acme`) | On purpose: `hivemind tenant create <id>` on SQLite; `POST /v1/tenants` with the admin key on Postgres. | Refused. Every ledger open (CLI, stdio MCP, HTTP server) checks the tenant on both backends and errors on an unknown one instead of opening an empty scope. |
| Shared project (`billing`) | On purpose, by anyone: `hivemind project register billing`. | Refused: ``project not registered: nosuch -- register it first with `hivemind project register nosuch` ``. No decision is recorded. |
| Personal project (`personal:human:alex`, `personal:agent:claude`) | Comes with the identity. There is nothing to register. | Cannot be a typo: it is derived from the actor, and a capture may not state one (`project must not use the reserved "personal:" prefix -- personal projects are derived from the actor, never stated: <handle>`). |

A tenant is chosen by where you connect (`--tenant`, `HIVEMIND_TENANT`, or the
token). A project is part of the decision itself, so it travels with every
capture and comes back with every answer.

**Personal projects.** The address is the actor id with any session removed:
`human:alex` becomes `personal:human:alex`, and `agent:claude:crew-1` becomes
`personal:agent:claude`, one per agent tool and never one per session. The
session stays on each decision as provenance, and `hivemind project decisions
personal:agent:claude` prints it on every row. An actor id of any other shape is
prefixed as it is. Personal projects are visible to the whole tenant and
labelled with their owner ("alex's personal project", "claude agents' personal
project"); privacy for them is not built. A capture that names no project lands
in its recorder's personal project and says so every time:

```text
saved to your personal project; pass a registered project handle to file it under a shared one
```

A decision recorded before projects existed has no project field and reads as
its recorder's personal project (`project_source` `personal_fallback`). That is
derived when the ledger is projected, so no event was rewritten and no migration
ran; such a decision moves like any other.

### The registry

Registering, linking, and anchoring are ledger events like any other, each with
its actor and time: `project.registered`, `project.linked`, `project.unlinked`,
`project.anchored`, `project.unanchored`, and `project.topic_declared` (see
[Topic vocabulary](#topic-vocabulary)). The registry is what replaying
them yields, so "who registered Billing, who linked it under Platform, and when"
is answered from the ledger the way a decision's provenance is. The commands
layer refuses, before anything is appended:

- a handle that is not lowercase letters, digits, and dashes, 2 to 40
  characters, or that starts with `personal:`;
- a handle that is already registered (the refusal names the existing project).
  Registration is permanent: there is no unregister event;
- a link with an unregistered end, a link from a project to itself, a second
  active `part_of` parent for one project, and an unlink of a link that is not
  active;
- an anchor on an unregistered project, a `rig` anchor whose value another
  project already holds, and removing an anchor that is not active.

The write layer does not prevent a `part_of` cycle; every read walks a chain
once and stops at the first project it would revisit. Removing an anchor exists
as an event (`project.unanchored`) but has no CLI verb yet.

The registry is read and written from the command line. Anyone can register a
project; there is no admin step.

| Verb | What it does |
| --- | --- |
| `hivemind project register <handle> [--display-name N] [--purpose P]` | Register a shared project. |
| `hivemind project link --from A --to B --kind part_of\|depends_on` and `project unlink` (same flags) | Record or retract a link. |
| `hivemind project anchor --handle H --kind folder\|rig\|jira\|linear\|github\|channel --value V` | Record an anchor. |
| `hivemind project declare-topic <handle> <key>...` or `--in-use` | Add topic keys to a project's vocabulary (see [Topic vocabulary](#topic-vocabulary)). |
| `hivemind project list` / `project show <handle>` | Read the registry (paged). An unknown handle is a successful reply with `outcome=not_found`; a `personal:` address always resolves. |
| `hivemind project decisions <handle-or-personal-address>` | The decisions in one project, oldest first, paged. On a personal address it is the review list: what is still in a personal project and not yet shared. |
| `hivemind project use <handle>` / `--clear` | Set the current project (see below). |

The registry verbs are not on MCP or REST. Only the move verb and the project
arguments described below are.

### Topic vocabulary

A topic says what a decision is about (`pricing`); a project says where it belongs
(`billing`). Left free, every capture invents its own keys and recall by topic
becomes a lottery, so a registered project has a **topic vocabulary**: the keys
declared for it, each declaration a `project.topic_declared` fact with its actor
and time. The vocabulary is what replaying them yields, and `hivemind project show
<handle>` lists it.

- Keys are normalised to lowercase kebab before anything else.
- A capture filed under a registered project may use only declared keys. A capture
  that uses another is refused before anything is written, naming the keys, what the
  project has, and how to declare.
- A vocabulary grows only when someone says so: a capture's `--declare-topic` (MCP
  `declare_topics`) for a key it uses, or `hivemind project declare-topic <handle>
  <key>...`. The reply lists what a capture declared. A key already declared is not
  declared twice, a new project starts empty, and nothing removes a key.
- A project that had decisions before it had a vocabulary adopts what they use with
  `hivemind project declare-topic <handle> --in-use`: one recorded declaration per
  key the project's decisions now carry.
- A personal project and a capture with no project have no vocabulary: any key is
  accepted, and declaring one is refused.
- A move never checks the destination's vocabulary. A decision keeps the keys it was
  captured with, and correcting where it lives must not be refused for them.

The rule is enforced by the write layer for every surface that names a project (CLI,
stdio MCP, MCP over HTTP). The REST route and the classifier's ingest name none, so
they file under the personal project and are unaffected. Checking a key against the
vocabulary replays the ledger, like the other registry checks.

### Links

Two kinds, no more:

- **`part_of`.** Billing is part of Platform. A project has at most one parent,
  and chains may be any depth. Platform's decisions reach Billing as inherited
  constraints, labelled `from Platform; Billing is part of it`.
- **`depends_on`.** Billing depends on Auth. A project may depend on any number
  of others. Auth's decisions reach Billing labelled `from Auth; Billing depends
  on it`.

Visibility flows one way and one hop. A lookup asked from Billing looks in
Billing, then its parent, then its dependencies, and nowhere else; the answer
carries a scope note that names every project looked in and counts what was not
followed (more levels up the `part_of` chain, more linked projects), so a short
answer never reads as a complete one. A parent does not see its children's
decisions by default. When a parent-level decision rests on a child's, it says
so explicitly, with `--rests-on-decision`, like any other premise. Links are
read from the ledger, so a link that was unlinked is not followed. See
[`AGENT_FLUENT_QUERYING.md`](AGENT_FLUENT_QUERYING.md) for the query side.

### Anchors

An anchor is a place in the world that says "decisions recorded from here belong
to this project". Six kinds can be recorded (`folder`, `rig`, `jira`, `linear`,
`github`, `channel`). Two ways of attaching a place to a project work today:

- **Folder marker.** A one-line file named `.hivemind-project` holding one
  handle, checked in with the code. It covers its folder and everything below
  it until a nearer marker; the nearest one wins. It is a file, not a registry
  entry: `hivemind project anchor --kind folder` records a fact about the
  project and does not create the file, and nothing reads a folder anchor back
  when resolving a capture. Overlap is not checked centrally: a folder has one
  marker file, and no shared table has to be edited to attach it.
- **Rig.** In a Gas City every session carries its rig in `GC_RIG`. A `rig`
  anchor whose value is that name binds the rig to a project. The value is
  unique per tenant.

`jira`, `linear`, `github`, and `channel` anchors can be recorded, but nothing
resolves a project from them yet. An agent that already knows the project from a
PM tool passes it with `--project`, and `--project-source job` says the handle
came from the job it was running.

### Where the project comes from

The client works out the project, in this order, first match wins, and records
how (`project_source`):

1. the project the caller stated (`stated`);
2. the `.hivemind-project` markers of the files the uncommitted change touches,
   else the nearest marker above the working directory (`folder_marker`);
3. the project anchored to the rig in `GC_RIG` (`rig`);
4. the current project, set once with `hivemind project use`
   (`current_project`), a per-machine setting kept in the CLI's `--hivemind-dir`,
   keyed by tenant and by the CLI's `--actor`, and never a ledger fact;
5. none of these: the recorder's personal project (`personal_fallback`), unless
   the session runs in a rig no project in this ledger is anchored to. That is a
   session writing to the wrong ledger, not a folder nobody attached, so a capture
   is refused and names the rig, the ledger, and the ways out (write to the ledger
   that holds the rig's project, anchor the rig here, or name the project); a
   supersede, whose project is the replaced decision's, is not.

A change that touches folders of several projects is one decision for the
nearest project they are all `part_of`, never one for each. With no project in
common it is saved to the recorder's personal project and the reply names the
projects it spans, because losing a decision is worse than misfiling one and a
move puts it right. A spanning change is never refused and never dropped.
[`AGENT_DECISION_CAPTURE.md`](AGENT_DECISION_CAPTURE.md) has the flags and the
exact replies.

This ladder lives once, in the client, and only runs when the caller asks for it
(`--project-from-context` on the CLI capture verbs and on the stdio
`hivemind mcp`). It is not set in stone: it is one small module behind one
function, so it can move into a plugin or server-side without touching the
write or query layers.

**Over HTTP the project is an argument, or absent.** A server has no view of the
caller's working directory, so the HTTP server never infers a project.
MCP-over-HTTP `capture_decision` and `supersede_decision` take `project` (and
`project_source`); only the CLI and the stdio MCP server fill it in from where
they run. The REST `POST /v1/decisions` route and the classifier's ingest take no
project yet: what REST records lands in the actor's personal project, and its
reply does not carry the fallback notice.

### What answers and exports show

Every decision an answer returns names its project (`get_decision`, `verify`,
`why`, `search`, `recall`, `situational`, `recent`, the compact view, and the
decision log): `project` (the address) and `project_label` (what a person calls
it). A decision that no proposal ever
recorded, only named by a request or a blocker, has `project: null` and the
label "no project recorded", so an unassigned decision is visible and never
guessed. `hivemind export --format markdown` writes the decision record grouped
per project: an `INDEX.md` with one section per project, and for each project a
`projects/<handle>/INDEX.md` plus one file per decision under
`projects/<handle>/decisions/`, personal projects under
`projects/personal/<actor>/`; `--project` limits it to one project.

### Moving a decision

A move is a `decision.moved` event: decision, from, to, an optional reason, the
actor, the time. The write layer requires that the decision exists, that `to` is
a registered project or the acting actor's own personal address (never someone
else's), and that `from` is where the decision is now. `hivemind move
"<description>" --to <handle>` and MCP `move_decision` (both transports) name
only the target; where the decision is now is read from the ledger, never typed.
The description goes through the same ambiguity gate as `supersede` and
`disagree`: several matches return numbered candidates and write nothing, and no
match is a successful reply with `outcome: not_found`.

A move reads as "moved from Billing to Pricing by Alex on ..." in the decision
history (`query get_recent_activity` and `get_decisions_changed_since` return a
`project_moved` row with the two ends, the actor, and the time). The decision's
own project changes and its `project_source` becomes `moved`. Reversal is
another move with the ends swapped; nothing is edited or deleted.

Finding what to move is a report, not a rule: `hivemind query scan_misfiled_decisions
--foreign-topic <key>` (MCP `scan_misfiled_decisions`) flags decisions carrying a
topic key the caller names as foreign to where they are filed. Each row says which
project the decision is filed under now; `--project` limits the report to decisions
filed exactly under one project (an unregistered handle is refused, never an empty
report), and `--move-to <project>` names where the flagged ones belong, leaving out
decisions already there and giving each row the `hivemind move --decision <id> --to
<project>` that puts it there. The report moves nothing; whoever confirmed the list
runs the moves, and a second run of the scoped report is clean.

### Sub-projects stay possible

Alex's standing constraint on the first slice: nothing may foreclose
sub-projects. What keeps them open today is that `part_of` is a tree of any
depth, that a marker nested inside an attached folder names a sub-project and
wins by being nearer, and that a scoped answer says how many `part_of` levels it
did not follow. What is not built is a first-class experience: inheritance across
several levels, nested markers registered as children, and a determination rule
that has been revisited for them.

### Not built

Waiting, on purpose: Jira, Linear, GitHub, and channel anchors that resolve a
project; bulk moves by anchor; splitting and merging projects with retired
aliases; suggestions for where a decision belongs; privacy for personal
projects; lookups deeper than one hop; and the registry verbs over MCP and REST.

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
