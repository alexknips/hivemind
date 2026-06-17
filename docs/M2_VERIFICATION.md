# M2 End-to-End Verification — Multi-Tenant Service

**Bead:** hivemind-m2-shared-backend-lives-uuq9.10  
**Outcome:** OPTION (b) — achieved with caveats (step 5 deferred)  
**Date verified:** 2026-06-17

---

## Verification Matrix

### Step 1: Tenant provisioning + per-tenant bearer keys ✅

**Dev mode (SQLite):** Tenant scoping is header-driven (`x-hivemind-tenant`). Each
tenant writes to and reads from an isolated ledger path. Verified by the
`rls_cross_tenant_decision_not_visible` HTTP API test.

**Production mode (Postgres):** `POST /v1/tenants` provisions a tenant and returns a
`hm_tk_<64-hex>` bearer token whose SHA-256 hash is stored in `hm_tokens`. The
`TenantStore::resolve_token` path looks up the tenant from the hash. Verified by
`tenant_store_tests::provision_creates_tenant_and_resolves_token` (requires
`TEST_DATABASE_URL`; skipped in CI without a real Postgres instance).

### Step 2: Ingest via POST /v1/ingest ✅

Verified by three tests in `tests/api.rs`:
- `ingest_batch_accepted_and_stored` — batch with turns round-trips and returns a
  `batch_id`.
- `ingest_batch_empty_turns_accepted` — empty turns array is valid.
- `ingest_batch_enforces_auth` — 401 when no bearer token and an API key is configured.

### Step 3: Query / read path ✅

Verified by `capture_and_query_decision` in `tests/api.rs`:
- `POST /v1/decisions` captures and returns a `decision_id`.
- `GET /v1/decisions/{id}` retrieves the same decision.
- `GET /v1/decisions/search?q=…` finds the decision by keyword.
- `GET /v1/decisions/relevant?topic=…` finds by topic key.

Supersession chain: `supersede_links_old_to_new_decision`.

### Step 4: RLS tenant isolation — tenant A cannot read tenant B ✅

**HTTP layer (SQLite dev mode):** `rls_cross_tenant_decision_not_visible` in
`tests/api.rs` — decision captured as tenant "alpha" returns `data: null` when
fetched as tenant "beta".

**Commands layer (SQLite):** Nine tests in `tests/multi_tenant.rs` verify that three
tenants (alpha, beta, gamma) plus the local tenant each see only their own events,
decisions, evidence, and hypotheses. Cross-tenant reads return zero results.

**Postgres RLS:** `tenant_store_tests::rls_prevents_cross_tenant_reads` confirms that
row-level security blocks cross-tenant event reads at the database layer (requires a
real Postgres instance with the `shared-backend-postgres` feature).

### Step 5: Migrate a local SQLite ledger in as a 3rd tenant ⏸ DEFERRED

The migration tool is tracked in **uuq9.8** (`hivemind migrate --from sqlite://… --to
postgres://… --tenant <name>`). That bead is not yet implemented. Step 5 is explicitly
out of scope for this verification. Acceptance of M2 is not blocked on it.

### Step 6: Auth enforcement + overall "service lives" e2e ✅

Auth tests in `tests/api.rs`:
- `auth_rejects_missing_token` — 401 when no `Authorization` header.
- `auth_accepts_correct_token` — 200 with the correct bearer token.
- `auth_rejects_wrong_token` — 401 with a well-formed but wrong token.

Docker build produces a runnable `hivemind serve` image. The `/v1/health` endpoint
returns `{"status":"ok"}` when the database is reachable. The Docker healthcheck polls
this endpoint. Verified via `docker build` completing without errors and the
`health_returns_ok` API test.

---

## Quality Gate Results

| Gate | Result |
|------|--------|
| `cargo +stable fmt --check` | ✅ PASS |
| `cargo clippy --locked --all-targets -- -D warnings` | ✅ PASS |
| `cargo test --locked` | ✅ PASS (196 tests, 0 failures, 2 ignored) |
| `cargo audit` | ✅ PASS (0 vulnerabilities; 2 allowed warnings in ratatui dep) |
| `docker build` | ✅ PASS |
| UBS critical count | ✅ 0 |

---

## Caveats

1. **Step 5 (migration) deferred.** No migration CLI exists yet. Tracked in uuq9.8.
2. **Postgres RLS tests require a real database.** The `tenant_store_tests` under
   `#[cfg(feature = "shared-backend-postgres")]` are skipped in CI unless
   `TEST_DATABASE_URL` is set. The SQLite-level multi_tenant tests provide equivalent
   isolation coverage for the dev path.
3. **Production bearer-token provisioning** is Postgres-only. SQLite dev mode uses
   header-based tenant scoping, which is intentionally simpler and not bearer-secured.
