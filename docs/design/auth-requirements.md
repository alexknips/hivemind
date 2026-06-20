# Auth & Tenancy Requirements

Status: requirements doc — captures the WHAT and WHY; see
[`AUTH_MODEL.md`](../AUTH_MODEL.md) for the accepted architectural decision on the HOW.

Source: requirements-gathering discussion between alex and mayor (2026-06-20),
consolidated here for the hivemind-ni7f bead.

---

## Context: Correcting a Wrong Assumption

Earlier thinking treated HiveMind's primary user as a **deployed, headless
agent** — a service token, a CI bot, or an autonomous fleet member calling the
API without any human in the loop.

**That assumption is wrong.** The primary user is an **interactive coding agent
with a human present** — Claude Code, Cursor, or a similar assistant running on
a developer's machine, with the developer at the keyboard. The human is there
at connect time and is able to complete a browser flow.

This single correction has cascading consequences for auth:
- **Browser OAuth is the right primary mechanism**, not static access tokens.
- **Session UX matters** because a real person experiences login friction.
- **Device/headless flow is secondary** — important but later, because most
  users have a browser.
- **Revocability and scope** should be designed for human oversight, not
  machine-to-machine trust.

---

## User Model

**Primary user:** an interactive coding agent (Claude Code / Cursor) running on
a developer's machine, with the developer present and able to act.

**Not the primary user:** deployed CI bots, headless automation fleets, or
server-side agents without a human in the loop. These will be supported, but
they are not the design center for auth.

---

## Tenancy

### Core model

Every piece of data — the decision ledger, login identities, connectors — hangs
off a **tenant**. A tenant is the top-level isolation boundary.

For an individual developer, a tenant is a **workspace of one**: one person, one
ledger, one set of connectors. When an organization comes in, it is the same
tenant model with membership and roles added on top — not a rebuild.

**Tenant-first means adding orgs is additive**, not structural. The data model
and auth flows must not assume solo users, even in the near term.

### Roadmap

| Phase | Who | Tenant shape |
|-------|-----|--------------|
| Now | Individual developers | Workspace-of-one |
| Soon | Small teams / orgs | Shared workspace with membership |

---

## Inbound Auth (Agent → HiveMind)

### Identity providers (login)

| Provider | When |
|----------|------|
| GitHub social login | Now |
| Google social login | Now |
| Google Workspace SSO (SAML/OIDC) | Later |
| Okta and other org SSO | Later |

The login path must accommodate SSO without re-plumbing the auth layer. SSO is
an additive identity provider, not a new auth architecture.

### Primary form: browser OAuth

When a developer runs Claude Code or Cursor and connects to HiveMind for the
first time, the flow is browser OAuth with a human completing the consent step:

1. Agent opens a browser to the HiveMind authorization endpoint.
2. Developer logs in with GitHub or Google.
3. HiveMind issues a **workspace-scoped session**.
4. Session lasts **one week**; on expiry, full browser re-auth is required.

Session renewal is not optimized — re-auth after a week is acceptable. The
ergonomic cost is low because the human is present by definition.

Sessions are **workspace-scoped**: they grant access to exactly one tenant. A
developer with multiple workspaces authenticates separately for each.

### Second form: device/headless authorization (later)

For agents that run where no browser can open (CI-like contexts, remote shells),
HiveMind will support a **device-code-style flow**:

1. Agent generates a short link and displays it.
2. Developer approves it on another device (phone, browser tab).
3. HiveMind issues a session tied to the device.

This is coarse-grained — it is a **setup/integration-level grant**, like
authorizing a Slack MCP integration. It is not per-action step-up approval.
Every device auth is still human-approved; no purely autonomous grant path
exists.

### What we are not doing

- **No static access tokens.** API keys, service tokens, or long-lived secrets
  are not part of the auth model. Auth is session-based with a human in the
  loop.
- **No per-action step-up.** After login, the session grants the configured
  scope for that workspace. There is no prompt-the-user flow for individual
  writes or reads.

---

## Outbound Connectors (Later)

Connectors are the flows that pull context *into* HiveMind from external sources
(Slack, Google Docs, etc.). They are what makes browser OAuth important
structurally: outbound grants are OAuth grants, and those require a human
present to approve.

### Grant model

| Phase | Scope |
|-------|-------|
| Individual workspaces | Per-user OAuth grants |
| Org workspaces | Per-workspace grants with membership |

### Sources (order not decided)

- Slack (channel activity, threads, decisions)
- Google Docs (documents linked to decision context)
- More sources TBD

Which connector ships first is an **open question** (see below).

---

## Architecture Constraints

### Sidecar, not baked into the Rust server

Auth — the OAuth authorization server, session management, and login flows —
lives in a **sidecar service**, not inside `hivemind serve`.

The Rust server's role is to be a thin **OAuth resource server**:
- Validate incoming tokens.
- Resolve the caller to a `tenant_id` and `actor_id`.
- Pass those as `RequestContext` into the commands and queries layer.

This boundary keeps the core server stable while the auth sidecar can evolve,
be swapped, or be hosted separately.

Part 1 (MCP-over-HTTP transport, commit e91dafb) has already landed and is held
unmerged, ready for the auth layer to connect to it.

### Provider: open decision

The OAuth authorization server provider is **not yet decided**:

- WorkOS (managed identity provider)
- Better Auth on Neon (auth framework + Postgres)
- Custom Rust broker

This is a downstream decision. Do not resolve it here. It should be surfaced as
a formal decision in HiveMind once the trade-offs are evaluated.

---

## Non-Goals

These are explicitly out of scope for this design:

- **Static access-token / API-key auth as the primary focus.** Machine tokens
  may appear later as a secondary form for specific integrations, but they are
  not the design center.
- **Per-action step-up approval.** Once a session is active, it grants its
  configured scope. Individual writes and reads do not prompt for additional
  confirmation.
- **Optimizing session renewal.** A one-week session expiring and requiring
  fresh browser auth is acceptable. Refresh-token flows, sliding sessions, and
  silent renewal are not prioritized.

---

## Open Questions

These are flagged for alex to resolve. Do not answer them here.

1. **Read vs. write scope.** Once an agent is authenticated, what can it do?
   The M3 MCP gateway is read-only today. When does write access become part of
   the session scope, and how is it configured?

2. **Which connector ships first.** Slack or Google Docs? The connector order
   affects which OAuth grant flows need to be ready and which integration surface
   gets tested first with real users.

3. **Account-linking.** If the same developer uses both GitHub and Google to
   log in (at different times or on different devices), are they the same
   workspace or two separate ones? How does HiveMind link those identities to
   one `principal_id`?

---

## Near-Term Scope Implied (Not a Commitment)

The requirements above sketch a natural build sequence:

1. **Inbound login first** — individual developers, GitHub/Google social login,
   browser OAuth, one-week workspace-scoped sessions, workspace-of-one tenancy.
2. **Device/headless flow** — device-code style, human-approved, for contexts
   without a browser.
3. **Outbound connectors** — Slack and/or Google Docs OAuth grants, per-user
   scope.
4. **Orgs and SSO** — membership and roles on top of the existing tenant model;
   Google Workspace/Okta SSO as additional identity providers.

Nothing in steps 2–4 requires rearchitecting the foundation from step 1. The
tenant-first model and sidecar architecture are chosen precisely because they
do not foreclose later additions.
