# WorkOS AuthKit Setup Runbook

**Purpose:** Provision WorkOS for the HiveMind MCP OAuth on-ramp (bead iwz9).
Our Rust server is a thin **resource server** — WorkOS AuthKit is the OAuth authorization
server (handles DCR, GitHub/Google login, token issuance). This runbook tells you exactly
what is dashboard-only versus scriptable via `workos-cli` / the Management API.

> **Current `.env` status (June 2026):** `WORKOS_CLIENT_ID` and `WORKOS_API_KEY` are already
> set. `WORKOS_DOMAIN`, `WORKOS_ISSUER`, and `WORKOS_JWKS_URL` are filled with the **User
> Management** issuer (`https://api.workos.com/user_management/<client_id>`) — valid for
> sealed-session validation but **incorrect for MCP OAuth tokens**. After completing Step 5
> below, update those three vars to the `authkit.app` domain values. `WORKOS_AUDIENCE` and
> `HIVEMIND_DATABASE_URL` still need to be filled in.

---

## Architecture Summary

```
MCP Client (Claude Code, Codex, Cursor)
  │  1. Discovers AS via /.well-known/oauth-authorization-server (proxied from our server)
  │  2. Registers via DCR: POST https://<authkit_domain>/oauth2/register
  │  3. OAuth PKCE flow → https://<authkit_domain>/oauth2/authorize → GitHub/Google login
  │  4. Token: POST https://<authkit_domain>/oauth2/token
  ↓
HiveMind Rust server  (resource server)
  │  Validates Bearer JWT: JWKS from https://<authkit_domain>/oauth2/jwks
  │  Checks iss == https://<authkit_domain>
  │  Maps WorkOS user sub → tenant (Postgres multi-tenant model)
  └→ MCP tools
```

**Key invariant:** No GitHub/Google OAuth app credentials live on our side. WorkOS manages
those IdP connections from its dashboard. We only need `WORKOS_DOMAIN` / `WORKOS_JWKS_URL`
/ `WORKOS_ISSUER` / `WORKOS_CLIENT_ID` to operate as a resource server.

---

## Step 1 — Create a WorkOS Account & Project

**Dashboard-only. Not scriptable.**

1. Sign up at <https://dashboard.workos.com>
2. Create a new project (or select your existing one)
3. Note your **Client ID** — shown on the dashboard home:
   `client_01XXXXXXXXXXXXXXXXXXXXXXXXX`

This is `WORKOS_CLIENT_ID` in our `.env`.

---

## Step 2 — Get API Key

**Dashboard-only. Not scriptable.**

1. Dashboard → **API Keys** (left nav)
2. Create a new secret key (or use the existing one)
3. Copy immediately — shown only once

This is `WORKOS_API_KEY` in our `.env`. Format: `sk_live_XXXX` (production) or
`sk_test_XXXX` (staging).

> **Note:** The current resource-server code does NOT read `WORKOS_API_KEY` — JWT
> validation is JWKS-only. The key is captured here for future hardening work
> (token introspection / revocation — bead hivemind-tk7n) and to enable social
> connection setup below.

---

## Step 3 — Enable Google Social Connection

**Dashboard-only for production. Staging has WorkOS-managed defaults.**

### Staging (testing only — no OAuth app required)

WorkOS provides WorkOS-managed default Google credentials in the staging environment.
Enable by visiting:
**Authentication → OAuth providers → Google → toggle Enable**

Staging tokens will show WorkOS branding on Google's consent screen. Sufficient
for initial integration testing without creating your own Google Cloud project.

### Production (required before launch)

You must create your own Google Cloud Console OAuth app:

1. Go to <https://console.cloud.google.com> → **APIs & Services → Credentials**
2. **Create Credentials → OAuth 2.0 Client ID**
   - Application type: **Web application**
3. In the WorkOS Dashboard: **Authentication → OAuth providers → Google → Manage**
4. Copy the **Redirect URI** shown in the WorkOS dialog
5. Back in Google Console, add that URI to **Authorized redirect URIs** → **Save**
6. Copy your **Client ID** and **Client Secret** from Google Console
7. Back in WorkOS: select **Your app's credentials**, paste both fields → **Save**
8. Toggle **Enable** if not already on

> The redirect URI WorkOS provides looks like:
> `https://api.workos.com/user_management/callback`

---

## Step 4 — Enable GitHub Social Connection

**Dashboard-only for production. Staging has WorkOS-managed defaults.**

### Staging

WorkOS may provide managed GitHub credentials in staging (confirmed for Google;
GitHub availability depends on your account tier — check the dashboard).
Enable: **Authentication → OAuth providers → GitHub → toggle Enable**

If managed credentials aren't available, proceed directly to the production steps.

### Production

Create a GitHub OAuth App (OAuth App is simpler; GitHub App gives refresh tokens):

1. GitHub → **Settings → Developer settings → OAuth Apps → New OAuth App**
2. Fill in:
   - Homepage URL: `https://your-hivemind-host.com`
   - Authorization callback URL: the **Redirect URI** from WorkOS (get it first — see step 3 pattern)
3. **Generate a new client secret** — copy immediately
4. WorkOS Dashboard: **Authentication → OAuth providers → GitHub → Manage**
5. Copy the Redirect URI from WorkOS, paste as GitHub's callback URL → **Update application**
6. Back in WorkOS: paste **Client ID** and **Client Secret** → **Save**
7. Toggle **Enable**

---

## Step 5 — Find Your AuthKit Domain

**Dashboard-only. This value drives most of our `.env`.**

1. Dashboard → **Connect → Configuration**
2. Your AuthKit domain appears here, format:
   `https://random-phrase-12345.authkit.app`

This is `WORKOS_DOMAIN` and `WORKOS_ISSUER` in our `.env`.

> **Critical:** For the MCP OAuth AS flow, tokens are issued by the AuthKit domain,
> NOT by `https://api.workos.com/user_management/<client_id>`. The current `.env`
> has the User Management issuer URL (used for sealed-session validation). Update it
> to the `authkit.app` domain for MCP JWT validation.
>
> Confirm the issuer by fetching the discovery document:
> ```bash
> curl https://your-project.authkit.app/.well-known/oauth-authorization-server | jq .issuer
> ```

---

## Step 6 — Enable Client ID Metadata Document (CIMD) + DCR

**Dashboard-only. Two toggles under Connect → Configuration.**

CIMD is the modern MCP client registration mechanism (recommended, 2025+ spec).
DCR is the legacy fallback (still needed for older MCP clients).

1. Dashboard → **Connect → Configuration**
2. Toggle **Client ID Metadata Document** → **Enable** *(do this first)*
3. Toggle **Dynamic Client Registration** → **Enable** *(for backwards compatibility)*

Once enabled, the DCR endpoint becomes live at:
`https://<authkit_domain>/oauth2/register`

MCP clients discover it via:
```bash
curl https://<authkit_domain>/.well-known/oauth-authorization-server | jq .registration_endpoint
```

---

## Step 7 — Add Resource Indicators

**Dashboard-only.** Scopes access tokens to your MCP server URL.

1. Dashboard → **Connect → Configuration → Resource Indicators**
2. Add your MCP server URL, e.g.:
   - Local: `http://localhost:8080/mcp`
   - Production: `https://hivemind.fly.dev/mcp`

This causes the issued access token's `aud` claim to match the resource URL,
enabling strict audience validation on the resource server side.

> Optional but recommended for production. Leave unset initially to skip aud
> validation while testing (our code already allows empty `WORKOS_AUDIENCE`).

---

## Step 8 — workos-cli (Limited Usefulness Here)

**The `workos-cli` cannot configure social connections or DCR.** Those are
dashboard-only.

The old `workos-cli` (archived Feb 2026) has been superseded by `workos/cli`,
which is primarily an AI-powered SDK installer plus org/user/role provisioning.
Useful commands for ops:

```bash
# Install
npm install -g @workos-inc/cli

# Configure with your API key
workos env add --name production --api-key sk_live_XXX

# Switch environments
workos env switch production

# Org and user management (not social/DCR)
workos setup-org      # one-shot org onboarding
workos seed           # YAML-based declarative provisioning
workos config redirect add https://your-server.com/auth/callback
```

**Bottom line:** Use the CLI for org/user seeding and redirect-URI management.
All social connection and DCR configuration requires the dashboard.

---

## Step 9 — Management API (Reference)

WorkOS exposes REST endpoints for user/org management but **not** for enabling
social connections or DCR configuration. No documented API endpoint exists for:

- Enabling/disabling Google or GitHub social connections
- Enabling/disabling DCR or CIMD
- Retrieving the AuthKit domain programmatically

All three require the dashboard. The Management API (`api.workos.com`) covers:
users, organizations, memberships, roles, invitations, sessions, and audit logs.

---

## Env Var Mapping

All values are read from the WorkOS Dashboard. None require API calls to derive (except
the `authkit_domain` discovery confirmation in Step 5).

| `.env` Variable | Source | Format / Notes |
|---|---|---|
| `WORKOS_DOMAIN` | Dashboard → Connect → Configuration → AuthKit domain | `https://random-phrase-12345.authkit.app` |
| `WORKOS_ISSUER` | Same as `WORKOS_DOMAIN` | Confirmed by fetching `/.well-known/oauth-authorization-server` `.issuer` |
| `WORKOS_JWKS_URL` | Derived from `WORKOS_DOMAIN` | `${WORKOS_DOMAIN}/oauth2/jwks` — note the legacy api.workos.com value used `/sso/jwks/<client_id>` (different path) |
| `WORKOS_CLIENT_ID` | Dashboard home or API Keys page | `client_01XXXXXXXXXXXXXXXXXXXXXXXXX` |
| `WORKOS_AUDIENCE` | Your MCP server URL (Step 7) or `WORKOS_CLIENT_ID` | Leave blank to skip aud check during testing |
| `WORKOS_API_KEY` | Dashboard → API Keys | `sk_live_XXX` (prod) / `sk_test_XXX` (staging) |
| `HIVEMIND_DATABASE_URL` | Neon dashboard | `postgres://user:pass@host/db?sslmode=require` |

**Shell snippet to derive JWKS from domain after setting WORKOS_DOMAIN:**
```bash
export WORKOS_DOMAIN=https://your-project.authkit.app
export WORKOS_ISSUER=$WORKOS_DOMAIN
export WORKOS_JWKS_URL=${WORKOS_DOMAIN}/oauth2/jwks

# Verify issuer matches
curl -s ${WORKOS_DOMAIN}/.well-known/oauth-authorization-server | jq '{issuer, jwks_uri, registration_endpoint}'
```

---

## Checklist — Minimum to Test MCP Login Flow

- [ ] WorkOS project created; Client ID noted
- [ ] API Key created; noted
- [ ] Google social connection enabled (staging default is fine for testing)
- [ ] GitHub social connection enabled (staging default is fine for testing)
- [ ] AuthKit domain noted (e.g. `https://your-project.authkit.app`)
- [ ] CIMD enabled (Connect → Configuration)
- [ ] DCR enabled (Connect → Configuration)
- [ ] `.env` updated: `WORKOS_DOMAIN`, `WORKOS_ISSUER`, `WORKOS_JWKS_URL`, `WORKOS_CLIENT_ID`, `WORKOS_API_KEY`
- [ ] Issuer verified via `/.well-known/oauth-authorization-server`
- [ ] (Optional) Resource indicator added for the MCP endpoint URL

For production (before user-facing launch):
- [ ] Google production OAuth app created; credentials entered in WorkOS
- [ ] GitHub production OAuth app created; credentials entered in WorkOS
- [ ] Custom AuthKit domain configured (if desired)
- [ ] `WORKOS_AUDIENCE` set to MCP server URL + resource indicator added

---

## WorkOS AuthKit Endpoints (Reference)

All relative to `WORKOS_DOMAIN`:

| Endpoint | URL |
|---|---|
| OAuth AS metadata | `${WORKOS_DOMAIN}/.well-known/oauth-authorization-server` |
| JWKS | `${WORKOS_DOMAIN}/oauth2/jwks` |
| Authorization | `${WORKOS_DOMAIN}/oauth2/authorize` |
| Token | `${WORKOS_DOMAIN}/oauth2/token` |
| DCR registration | `${WORKOS_DOMAIN}/oauth2/register` |
| Token introspection | `${WORKOS_DOMAIN}/oauth2/introspection` |

Our MCP server must expose at `/`:

| Endpoint | Description |
|---|---|
| `/.well-known/oauth-protected-resource` | Points to WorkOS as the AS |
| `/.well-known/oauth-authorization-server` | Proxy of WorkOS AS metadata (for MCP clients that look here) |

These are implemented in bead iwz9.
