// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::DateTime;
use uuid::Uuid;

use crate::error::LedgerError;
use crate::events::TenantId;
use crate::ledger::contract_tests::{
    assert_dedup_by_event_uuid, assert_monotonic_append, assert_read_offset_and_limit,
    assert_replay_from_zero_in_order, make_event,
};
use crate::ledger::{EventLedger, SqliteEventLedger};
use crate::Result;

use super::PostgresEventLedger;

const TEST_DATABASE_URL_ENV: &str = "HIVEMIND_TEST_POSTGRES_URL";

#[test]
fn append_assigns_monotonic_ids() -> Result<()> {
    with_postgres_ledger("append-monotonic", assert_monotonic_append)
}

#[test]
fn append_is_idempotent_for_duplicate_event_uuid() -> Result<()> {
    with_postgres_ledger("append-dedup", assert_dedup_by_event_uuid)
}

#[test]
fn replay_from_zero_is_ordered() -> Result<()> {
    with_postgres_ledger("replay-ordered", |ledger| {
        assert_replay_from_zero_in_order(ledger)
    })
}

#[test]
fn read_applies_offset_and_limit() -> Result<()> {
    with_postgres_ledger("read-offset-limit", |ledger| {
        assert_read_offset_and_limit(ledger)
    })
}

#[test]
fn same_event_uuid_is_idempotent_only_inside_tenant() -> Result<()> {
    with_postgres_ledger("tenant-dedup-a", |tenant_a| {
        let tenant_b = tenant_a.for_tenant(unique_tenant("tenant-dedup-b"))?;
        let event_uuid = Uuid::new_v4();

        let tenant_a_id = tenant_a.append(make_event("tenant-a-event", event_uuid))?;
        let tenant_b_id = tenant_b.append(make_event("tenant-b-event", event_uuid))?;
        let tenant_a_events = tenant_a.read(0, 10)?;
        let tenant_b_events = tenant_b.read(0, 10)?;
        let Some(tenant_a_event) = tenant_a_events.first() else {
            return Err(test_error("tenant-a event missing"));
        };
        let Some(tenant_b_event) = tenant_b_events.first() else {
            return Err(test_error("tenant-b event missing"));
        };

        if tenant_a_id != 1 {
            return Err(test_error("tenant-a first event id mismatch"));
        }
        if tenant_b_id != 1 {
            return Err(test_error("tenant-b first event id mismatch"));
        }
        if payload_evidence_id(tenant_a_event) != "tenant-a-event" {
            return Err(test_error("tenant-a payload mismatch"));
        }
        if payload_evidence_id(tenant_b_event) != "tenant-b-event" {
            return Err(test_error("tenant-b payload mismatch"));
        }

        Ok(())
    })
}

#[test]
fn replay_matches_sqlite_event_stream() -> Result<()> {
    with_postgres_ledger("sqlite-parity", |postgres| {
        let sqlite_dir = temp_hivemind_dir("sqlite-parity");
        let sqlite = SqliteEventLedger::open(&sqlite_dir)?;
        // Use the same tenant for SQLite so the read-back tenant_id field matches.
        let pg_tenant = TenantId::new(postgres.tenant_id())
            .map_err(|e| test_error(format!("invalid postgres tenant_id: {e}")))?;

        const EVIDENCE_IDS: [&str; 5] = [
            "evidence-0",
            "evidence-1",
            "evidence-2",
            "evidence-3",
            "evidence-4",
        ];
        for (index, evidence_id) in EVIDENCE_IDS.into_iter().enumerate() {
            let event = make_parity_event(evidence_id, index);
            sqlite.append_for_tenant(&pg_tenant, event)?;
        }

        let expected_events = sqlite.read_for_tenant(&pg_tenant, 0, 16)?;
        for event in expected_events.clone() {
            postgres.append(event)?;
        }

        if expected_events != postgres.read(0, 16)? {
            return Err(test_error("postgres event stream differs from sqlite"));
        }

        let _ = fs::remove_dir_all(sqlite_dir);
        Ok(())
    })
}

#[test]
fn concurrent_tenant_writes_are_isolated_streams() -> Result<()> {
    with_postgres_ledger("concurrent-a", |tenant_a| {
        let tenant_a = tenant_a.clone();
        let tenant_b = tenant_a.for_tenant(unique_tenant("concurrent-b"))?;

        std::thread::scope(|scope| {
            let left = scope.spawn(|| append_events(&tenant_a, "tenant-a", 16));
            let right = scope.spawn(|| append_events(&tenant_b, "tenant-b", 16));

            left.join()
                .map_err(|_| test_error("tenant-a thread panicked"))??;
            right
                .join()
                .map_err(|_| test_error("tenant-b thread panicked"))??;
            Ok::<_, crate::HivemindError>(())
        })?;

        assert_tenant_stream(&tenant_a, "tenant-a", 16)?;
        assert_tenant_stream(&tenant_b, "tenant-b", 16)?;

        Ok(())
    })
}

fn with_postgres_ledger<T>(
    prefix: &str,
    f: impl FnOnce(&PostgresEventLedger) -> Result<T>,
) -> Result<()> {
    let Some(database_url) = std::env::var(TEST_DATABASE_URL_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        eprintln!("skipping Postgres ledger test; set {TEST_DATABASE_URL_ENV}");
        return Ok(());
    };

    let tenant_id = unique_tenant(prefix);
    let ledger = PostgresEventLedger::connect_with_pool_size(&database_url, tenant_id, 4)?;
    f(&ledger)?;
    Ok(())
}

fn append_events(ledger: &PostgresEventLedger, prefix: &str, count: usize) -> Result<()> {
    let mut evidence_id = String::with_capacity(prefix.len() + 20);
    for index in 0..count {
        evidence_id.clear();
        let _ = write!(&mut evidence_id, "{prefix}-{index}");
        ledger.append(make_event(&evidence_id, Uuid::new_v4()))?;
    }
    Ok(())
}

fn assert_tenant_stream(
    ledger: &PostgresEventLedger,
    expected_prefix: &str,
    expected_count: usize,
) -> Result<()> {
    let events = ledger.read(0, expected_count + 1)?;
    if events.len() != expected_count {
        return Err(test_error("tenant stream event count mismatch"));
    }
    for (index, event) in events.iter().enumerate() {
        let expected_event_id =
            u64::try_from(index + 1).map_err(|_| test_error("event index out of range"))?;
        if event.event_id != Some(expected_event_id) {
            return Err(test_error("tenant stream event id mismatch"));
        }
        if !payload_evidence_id(event).starts_with(expected_prefix) {
            return Err(test_error("tenant stream payload prefix mismatch"));
        }
    }
    Ok(())
}

fn payload_evidence_id(event: &crate::events::Event) -> &str {
    event
        .payload
        .get("evidence_id")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
}

fn make_parity_event(evidence_id: &str, index: usize) -> crate::events::Event {
    let mut event = make_event(evidence_id, Uuid::new_v4());
    let seconds = i64::try_from(index).unwrap_or(0);
    event.ts = DateTime::from_timestamp(1_769_385_600 + seconds, 123_456_000);
    event
}

fn unique_tenant(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    format!("tenant:test:{prefix}:{nanos}:{}", std::process::id())
}

fn temp_hivemind_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    std::env::temp_dir().join(format!("hivemind-{prefix}-{nanos}-{}", std::process::id()))
}

fn test_error(message: impl Into<String>) -> crate::HivemindError {
    LedgerError::Storage(message.into()).into()
}

// ---------------------------------------------------------------------------
// TenantStore + RLS isolation tests (uuq9.22)
// ---------------------------------------------------------------------------

#[cfg(feature = "shared-backend-postgres")]
mod tenant_store_tests {
    use super::*;
    use crate::ledger::TenantStore;

    #[test]
    fn provision_creates_tenant_and_resolves_token() -> Result<()> {
        with_tenant_store(|store| {
            let tid = unique_tenant("provision-resolve");
            let provisioned = store
                .provision_tenant(&tid, "Test Org")
                .map_err(|e| test_error(e.to_string()))?;

            if provisioned.tenant_id != tid {
                return Err(test_error("tenant_id mismatch after provision"));
            }
            if !provisioned.token_secret.starts_with("hm_tk_") {
                return Err(test_error("token_secret missing prefix"));
            }

            let resolved = store
                .resolve_token(&provisioned.token_secret)
                .map_err(|e| test_error(e.to_string()))?;

            if resolved.as_deref() != Some(tid.as_str()) {
                return Err(test_error("resolved tenant_id does not match"));
            }
            Ok(())
        })
    }

    #[test]
    fn unknown_token_resolves_to_none() -> Result<()> {
        with_tenant_store(|store| {
            let resolved = store
                .resolve_token(
                    "hm_tk_000000000000000000000000000000000000000000000000000000000000abcd",
                )
                .map_err(|e| test_error(e.to_string()))?;
            if resolved.is_some() {
                return Err(test_error("unknown token should resolve to None"));
            }
            Ok(())
        })
    }

    #[test]
    fn rls_prevents_cross_tenant_reads() -> Result<()> {
        let Some(database_url) = std::env::var(TEST_DATABASE_URL_ENV)
            .ok()
            .filter(|v| !v.trim().is_empty())
        else {
            eprintln!("skipping RLS isolation test; set {TEST_DATABASE_URL_ENV}");
            return Ok(());
        };

        let store = TenantStore::connect(&database_url).map_err(|e| test_error(e.to_string()))?;

        // Provision two tenants.
        let tid_a = unique_tenant("rls-alice");
        let tid_b = unique_tenant("rls-bob");
        let _ = store
            .provision_tenant(&tid_a, "Alice Org")
            .map_err(|e| test_error(e.to_string()))?;
        let _ = store
            .provision_tenant(&tid_b, "Bob Org")
            .map_err(|e| test_error(e.to_string()))?;

        // Write one event per tenant through the ledger.
        let ledger_a = PostgresEventLedger::connect_with_pool_size(&database_url, &tid_a, 2)
            .map_err(|e| test_error(e.to_string()))?;
        let ledger_b = ledger_a
            .for_tenant(&tid_b)
            .map_err(|e| test_error(e.to_string()))?;

        ledger_a.append(make_event("alice-secret", Uuid::new_v4()))?;
        ledger_b.append(make_event("bob-secret", Uuid::new_v4()))?;

        // Alice's ledger should see only Alice's event.
        let alice_events = ledger_a.read(0, 10)?;
        if alice_events.len() != 1 {
            return Err(test_error(format!(
                "Alice expected 1 event, got {}",
                alice_events.len()
            )));
        }
        let alice_payload = alice_events[0]
            .payload
            .get("evidence_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if alice_payload != "alice-secret" {
            return Err(test_error(format!(
                "Alice event payload mismatch: {alice_payload}"
            )));
        }

        // Bob's ledger should see only Bob's event.
        let bob_events = ledger_b.read(0, 10)?;
        if bob_events.len() != 1 {
            return Err(test_error(format!(
                "Bob expected 1 event, got {}",
                bob_events.len()
            )));
        }
        let bob_payload = bob_events[0]
            .payload
            .get("evidence_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if bob_payload != "bob-secret" {
            return Err(test_error(format!(
                "Bob event payload mismatch: {bob_payload}"
            )));
        }

        Ok(())
    }

    fn with_tenant_store<T>(f: impl FnOnce(&TenantStore) -> Result<T>) -> Result<()> {
        let Some(database_url) = std::env::var(TEST_DATABASE_URL_ENV)
            .ok()
            .filter(|v| !v.trim().is_empty())
        else {
            eprintln!("skipping TenantStore test; set {TEST_DATABASE_URL_ENV}");
            return Ok(());
        };
        let store = TenantStore::connect(&database_url).map_err(|e| test_error(e.to_string()))?;
        f(&store)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Regression tests for hivemind-e8zp: extract_ctx async-safety
// ---------------------------------------------------------------------------
//
// Root cause: resolve_or_create_oidc_user / resolve_token call r2d2's pool.get()
// (and the native-tls handshake during connection setup) on the Tokio async
// worker thread. This triggers "Cannot start a runtime from within a runtime"
// in the connection machinery and aborts the server process with SIGABRT.
//
// Fix: extract_ctx is now `async fn` and wraps both blocking calls in
// tokio::task::spawn_blocking, matching the pattern used at every other DB
// call site in api.rs (14 sites were already correct; extract_ctx was the
// sole offender).
// ---------------------------------------------------------------------------

#[cfg(feature = "shared-backend-postgres")]
mod async_safety_tests {
    use crate::ledger::TenantStore;

    const PG_URL_ENV: &str = "HIVEMIND_TEST_POSTGRES_URL";

    fn require_pg_url() -> String {
        std::env::var(PG_URL_ENV).unwrap_or_else(|_| {
            panic!(
                "HIVEMIND_TEST_POSTGRES_URL must be set to run async-safety tests \
                 (e.g. postgresql://user:pw@localhost/db?sslmode=disable). \
                 A test that skips when Postgres is absent is not acceptable proof."
            )
        })
    }

    /// FAIL-BEFORE (class demonstration): calling block_on from inside a
    /// running Tokio runtime panics with "Cannot start a runtime from within
    /// a runtime". This is the exact panic class the production server hit
    /// when resolve_or_create_oidc_user (which internally goes through the
    /// connection pool's blocking machinery) was called directly on the async
    /// worker thread.
    #[tokio::test(flavor = "multi_thread")]
    async fn fail_before_runtime_within_runtime_panics() {
        let caught = std::panic::catch_unwind(|| {
            // Directly mirrors what the old extract_ctx did: block inside async.
            // In production, this fires through the native-tls TLS handshake
            // path in r2d2's connection creation.
            let rt = tokio::runtime::Runtime::new().expect("runtime build");
            rt.block_on(std::future::ready(()));
        });
        assert!(
            caught.is_err(),
            "block_on inside a running runtime MUST panic — if this assertion fails, \
             the test environment does not reproduce the production failure class"
        );
    }

    /// PASS-AFTER: the fixed path — wrapping the blocking pool call in
    /// spawn_blocking — succeeds on a multi-thread Tokio runtime backed by
    /// real Postgres. This is the exact code path now in extract_ctx.
    ///
    /// Key: ALL blocking I/O (connect, query, AND drop) must stay on the
    /// blocking thread. The postgres Client::drop calls block_on internally
    /// (postgres-0.19.14/src/connection.rs:66), so even an Arc drop on the
    /// async worker thread triggers the same SIGABRT. In production,
    /// AppState is created pre-tokio (safe); this test validates the per-
    /// request spawn_blocking pattern that is the actual fix.
    #[tokio::test(flavor = "multi_thread")]
    async fn pass_after_spawn_blocking_is_async_safe() {
        let db_url = require_pg_url();

        // Fixed path: connect + query + drop all on the blocking thread.
        // The spawn_blocking closure owns the TenantStore, so its Drop runs
        // on the blocking thread — no "Cannot start a runtime from within a
        // runtime" panic.
        let tenant_id = tokio::task::spawn_blocking(move || {
            let store = TenantStore::connect(&db_url).expect("TenantStore::connect failed");
            let result = store.resolve_or_create_oidc_user(
                "test-oidc-sub-e8zp-regression",
                "e8zp-regression@example.com",
            );
            drop(store); // explicit: store.drop runs here on the blocking thread
            result
        })
        .await
        .expect("spawn_blocking join must not panic")
        .expect("resolve_or_create_oidc_user must succeed");

        assert!(
            tenant_id.starts_with("oidc:"),
            "OIDC tenant_id must carry 'oidc:' prefix, got: {tenant_id}"
        );
    }
}

// ---------------------------------------------------------------------------
// Regression tests for hivemind-noc9: AppState/pool drop on scale-to-zero
// ---------------------------------------------------------------------------
//
// Root cause: on Fly autostop (scale-to-zero), SIGINT → graceful shutdown →
// AppState (which owns the r2d2 postgres pool) is dropped while main is still
// inside Runtime::block_on (the axum serve future). postgres::Client::drop
// calls block_on internally; when that runs inside an existing block_on context
// the tokio thread-local detects the nesting and panics with "Cannot start a
// runtime from within a runtime" → SIGABRT.
//
// Fix (src/cli/mod.rs run_serve): hold `_pg_guard = state.clone()` before
// creating the runtime. Rust drops locals in reverse declaration order, so
// `runtime` (declared after `_pg_guard`) drops first — removing the runtime
// context. Only then does `_pg_guard` drop, allowing the pool Drop to call
// block_on safely.
// ---------------------------------------------------------------------------

mod shutdown_drop_safety_tests {
    /// Simulates a pool whose Drop calls block_on internally (as postgres does).
    struct PoolDropCallsBlockOn;
    impl Drop for PoolDropCallsBlockOn {
        fn drop(&mut self) {
            // Mirrors postgres::Client::drop: create a separate runtime and
            // drive it to completion. Panics with "Cannot start a runtime from
            // within a runtime" when the current thread is inside an existing
            // tokio runtime context.
            let inner = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("inner rt build");
            inner.block_on(std::future::ready(()));
        }
    }

    /// FAIL-BEFORE (noc9 class): dropping the pool INSIDE block_on panics.
    /// The future holds `_pool`; when block_on drives the future to completion
    /// and drops it, `_pool` drops inside the tokio runtime context.
    /// pool::Drop → inner block_on → "Cannot start a runtime from within a runtime".
    #[test]
    #[should_panic(expected = "Cannot start a runtime from within a runtime")]
    fn fail_before_pool_drop_inside_block_on_panics() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("outer rt");
        // _pool is moved into the future; it drops at the end of the async block,
        // which executes inside block_on's runtime context.
        runtime.block_on(async {
            let _pool = PoolDropCallsBlockOn;
            std::future::ready(()).await;
        });
    }

    /// PASS-AFTER (noc9 fix): keeping a clone guard ensures the pool Drop runs
    /// AFTER the runtime is gone. Rust's reverse-declaration drop order:
    /// `runtime` (declared 2nd) drops first, then `_pg_guard` (declared 1st).
    #[test]
    fn pass_after_pool_drop_after_runtime_exits() {
        // Guard declared before runtime → drops after runtime (reverse order).
        let _pg_guard = PoolDropCallsBlockOn;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("outer rt");
        // runtime's block_on completes; the serve future (which would have held
        // the pool) is gone. runtime drops first, then _pg_guard drops.
        runtime.block_on(std::future::ready(()));
        // _pg_guard drops here: no runtime context → pool Drop → inner block_on → OK.
    }
}
