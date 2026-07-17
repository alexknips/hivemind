use std::error::Error as _;
use std::fmt;
use std::str::FromStr;
use std::thread;
use std::time::Duration;

use chrono::{DateTime, Utc};
use postgres::error::SqlState;
use postgres::{Config, Row, Transaction};
use postgres_native_tls::MakeTlsConnector;
use r2d2::Pool;
use r2d2_postgres::PostgresConnectionManager;
use serde_json::Value;
use uuid::Uuid;

use crate::events::{Event, EventId, EventSource, EventType, TenantId};
use crate::Result;

use super::backend_error::storage_error;
use super::EventLedger;

mod tenant_store;
pub use tenant_store::{ProvisionedUser, ResolvedToken, TenantStore, UserInfo};

const DEFAULT_POOL_SIZE: u32 = 16;
const MAX_TRANSIENT_RETRIES: usize = 3;
const RETRY_BASE_DELAY: Duration = Duration::from_millis(25);

type PgManager = PostgresConnectionManager<MakeTlsConnector>;
type PgPool = Pool<PgManager>;

/// Postgres-backed event ledger scoped to one tenant by default.
///
/// The underlying table is tenant-aware; cloning this type or calling
/// `for_tenant` reuses the same connection pool with a different tenant scope.
#[derive(Clone)]
pub struct PostgresEventLedger {
    pool: PgPool,
    tenant_id: String,
}

impl std::fmt::Debug for PostgresEventLedger {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PostgresEventLedger")
            .field("tenant_id", &self.tenant_id)
            .finish_non_exhaustive()
    }
}

impl PostgresEventLedger {
    pub const LOCAL_DEFAULT_TENANT_ID: &'static str = "tenant:local-default";

    pub fn connect(database_url: &str, tenant_id: impl Into<String>) -> Result<Self> {
        Self::connect_with_pool_size(database_url, tenant_id, DEFAULT_POOL_SIZE)
    }

    pub fn connect_local_default(database_url: &str) -> Result<Self> {
        Self::connect(database_url, Self::LOCAL_DEFAULT_TENANT_ID)
    }

    pub fn connect_with_pool_size(
        database_url: &str,
        tenant_id: impl Into<String>,
        max_size: u32,
    ) -> Result<Self> {
        let config = Config::from_str(database_url).map_err(storage_error)?;
        let tls = MakeTlsConnector::new(native_tls::TlsConnector::new().map_err(storage_error)?);
        let manager = PostgresConnectionManager::new(config, tls);
        let pool = Pool::builder()
            .max_size(max_size)
            .build(manager)
            .map_err(storage_error)?;

        Self::from_pool(pool, tenant_id)
    }

    fn from_pool(pool: PgPool, tenant_id: impl Into<String>) -> Result<Self> {
        let tenant_id = validate_tenant_id(tenant_id.into())?;
        let ledger = Self { pool, tenant_id };
        ledger.initialize_schema()?;
        Ok(ledger)
    }

    /// Create a ledger scoped to `tenant_id` sharing an existing pool (no schema init).
    pub fn from_shared_pool(pool: PgPool, tenant_id: impl Into<String>) -> Result<Self> {
        let tenant_id = validate_tenant_id(tenant_id.into())?;
        Ok(Self { pool, tenant_id })
    }

    /// Expose the underlying pool so callers can share it with a `TenantStore`.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub fn for_tenant(&self, tenant_id: impl Into<String>) -> Result<Self> {
        Ok(Self {
            pool: self.pool.clone(),
            tenant_id: validate_tenant_id(tenant_id.into())?,
        })
    }

    pub fn tenant_id(&self) -> &str {
        &self.tenant_id
    }

    pub fn append_for_tenant(&self, tenant_id: &str, event: Event) -> Result<EventId> {
        validate_tenant_id_ref(tenant_id)?;
        let stored = StoredEvent::from_event(event)?;

        self.with_retrying_transaction(|transaction| {
            set_tenant_local_pg(transaction, tenant_id)?;
            transaction.query_one("SELECT pg_advisory_xact_lock(hashtext($1))", &[&tenant_id])?;

            if let Some(existing_id) =
                existing_event_id(transaction, tenant_id, &stored.event_uuid)?
            {
                return Ok(existing_id);
            }

            let next_id = next_event_id(transaction, tenant_id)?;
            let inserted = transaction.execute(
                "INSERT INTO events (
                    tenant_id,
                    event_id,
                    event_uuid,
                    correlation_id,
                    causation_event_id,
                    event_type,
                    actor_id,
                    source,
                    source_ref,
                    payload,
                    ts
                ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
                ON CONFLICT (tenant_id, event_uuid) DO NOTHING",
                &[
                    &tenant_id,
                    &event_id_to_i64(next_id, "event_id")?,
                    &stored.event_uuid,
                    &stored.correlation_id,
                    &stored.causation_event_id,
                    &stored.event_type,
                    &stored.actor_id,
                    &stored.source,
                    &stored.source_ref,
                    &stored.payload,
                    &stored.ts,
                ],
            )?;

            if inserted == 1 {
                return Ok(next_id);
            }

            existing_event_id(transaction, tenant_id, &stored.event_uuid)?.ok_or_else(|| {
                PgOperationError::Storage(
                    storage_error(
                        "event dedup failed: duplicate event_uuid not found after INSERT",
                    )
                    .into(),
                )
            })
        })
    }

    pub fn read_for_tenant(
        &self,
        tenant_id: &str,
        offset: EventId,
        limit: usize,
    ) -> Result<Vec<Event>> {
        validate_tenant_id_ref(tenant_id)?;
        if limit == 0 {
            return Ok(Vec::new());
        }

        let offset = event_id_to_i64(offset, "offset")?;
        let limit = i64::try_from(limit)
            .map_err(|error| storage_error(format!("limit out of range: {error}")))?;
        let mut client = self.pool.get().map_err(storage_error)?;
        let mut tx = client.transaction().map_err(storage_error)?;
        set_tenant_local_pg(&mut tx, tenant_id).map_err(pg_op_to_result)?;
        let rows = tx
            .query(&read_events_sql(), &[&tenant_id, &offset, &limit])
            .map_err(storage_error)?;
        tx.commit().map_err(storage_error)?;

        rows.iter().map(event_from_row).collect()
    }

    pub fn replay_from_for_tenant(
        &self,
        tenant_id: &str,
        offset: EventId,
        callback: &mut dyn FnMut(&Event) -> Result<()>,
    ) -> Result<()> {
        validate_tenant_id_ref(tenant_id)?;
        let offset = event_id_to_i64(offset, "offset")?;
        let mut client = self.pool.get().map_err(storage_error)?;
        let mut tx = client.transaction().map_err(storage_error)?;
        set_tenant_local_pg(&mut tx, tenant_id).map_err(pg_op_to_result)?;
        let rows = tx
            .query(&replay_events_sql(), &[&tenant_id, &offset])
            .map_err(storage_error)?;
        tx.commit().map_err(storage_error)?;

        for row in &rows {
            let event = event_from_row(row)?;
            callback(&event)?;
        }

        Ok(())
    }

    pub fn latest_offset_for_tenant(&self, tenant_id: &str) -> Result<EventId> {
        validate_tenant_id_ref(tenant_id)?;
        let mut client = self.pool.get().map_err(storage_error)?;
        let mut tx = client.transaction().map_err(storage_error)?;
        set_tenant_local_pg(&mut tx, tenant_id).map_err(pg_op_to_result)?;
        let offset: Option<i64> = tx
            .query_one(
                "SELECT MAX(event_id) FROM events WHERE tenant_id = $1",
                &[&tenant_id],
            )
            .map_err(storage_error)?
            .get(0);
        tx.commit().map_err(storage_error)?;

        i64_to_event_id(offset.unwrap_or_default(), "latest_offset")
    }

    fn initialize_schema(&self) -> Result<()> {
        let mut client = self.pool.get().map_err(storage_error)?;
        client
            .batch_execute(
                "CREATE TABLE IF NOT EXISTS events (
                    tenant_id text NOT NULL,
                    event_id bigint NOT NULL,
                    event_uuid uuid NOT NULL,
                    correlation_id text,
                    causation_event_id bigint,
                    event_type text NOT NULL,
                    actor_id text NOT NULL,
                    source text NOT NULL DEFAULT 'cli',
                    source_ref text,
                    payload jsonb NOT NULL,
                    ts timestamptz NOT NULL,
                    PRIMARY KEY (tenant_id, event_id),
                    UNIQUE (tenant_id, event_uuid)
                );
                CREATE INDEX IF NOT EXISTS events_tenant_ts_idx
                    ON events (tenant_id, ts);
                CREATE INDEX IF NOT EXISTS events_tenant_type_idx
                    ON events (tenant_id, event_type);",
            )
            .map_err(storage_error)?;
        Ok(())
    }

    fn with_retrying_transaction<T>(
        &self,
        mut operation: impl FnMut(&mut Transaction<'_>) -> std::result::Result<T, PgOperationError>,
    ) -> Result<T> {
        let mut attempt = 0;
        loop {
            let outcome: std::result::Result<T, PgOperationError> = (|| {
                let mut client = self.pool.get().map_err(PgOperationError::Pool)?;
                let mut transaction = client.transaction().map_err(PgOperationError::Postgres)?;
                let value = operation(&mut transaction)?;
                transaction.commit().map_err(PgOperationError::Postgres)?;
                Ok(value)
            })();

            match outcome {
                Ok(value) => return Ok(value),
                Err(error) if attempt < MAX_TRANSIENT_RETRIES && error.is_transient() => {
                    thread::sleep(RETRY_BASE_DELAY * (attempt as u32 + 1));
                    attempt += 1;
                }
                Err(error) => return Err(storage_error(error).into()),
            }
        }
    }
}

impl EventLedger for PostgresEventLedger {
    fn append_for_tenant(&self, tenant_id: &TenantId, event: Event) -> Result<EventId> {
        self.append_for_tenant(tenant_id.as_str(), event)
    }

    fn read_for_tenant(
        &self,
        tenant_id: &TenantId,
        offset: EventId,
        limit: usize,
    ) -> Result<Vec<Event>> {
        self.read_for_tenant(tenant_id.as_str(), offset, limit)
    }

    fn replay_from_for_tenant(
        &self,
        tenant_id: &TenantId,
        offset: EventId,
        callback: &mut dyn FnMut(&Event) -> Result<()>,
    ) -> Result<()> {
        self.replay_from_for_tenant(tenant_id.as_str(), offset, callback)
    }

    fn latest_offset_for_tenant(&self, tenant_id: &TenantId) -> Result<EventId> {
        self.latest_offset_for_tenant(tenant_id.as_str())
    }

    // Override defaults: self.tenant_id may differ from TenantId::local().
    fn append(&self, event: Event) -> Result<EventId> {
        self.append_for_tenant(&self.tenant_id, event)
    }

    fn read(&self, offset: EventId, limit: usize) -> Result<Vec<Event>> {
        self.read_for_tenant(&self.tenant_id, offset, limit)
    }

    fn replay_from(
        &self,
        offset: EventId,
        callback: &mut dyn FnMut(&Event) -> Result<()>,
    ) -> Result<()> {
        self.replay_from_for_tenant(&self.tenant_id, offset, callback)
    }

    fn latest_offset(&self) -> Result<EventId> {
        self.latest_offset_for_tenant(&self.tenant_id)
    }
}

struct StoredEvent {
    event_uuid: Uuid,
    event_type: &'static str,
    actor_id: String,
    source: &'static str,
    source_ref: Option<String>,
    correlation_id: Option<String>,
    causation_event_id: Option<i64>,
    payload: Value,
    ts: DateTime<Utc>,
}

impl StoredEvent {
    fn from_event(event: Event) -> Result<Self> {
        let causation_event_id = event
            .causation_event_id
            .map(|id| event_id_to_i64(id, "causation_event_id"))
            .transpose()?;

        Ok(Self {
            event_uuid: event.event_uuid,
            event_type: event_type_as_str(event.event_type),
            actor_id: event.actor_id,
            source: event.source.as_str(),
            source_ref: event.source_ref,
            correlation_id: event.correlation_id,
            causation_event_id,
            payload: event.payload,
            ts: event.ts.unwrap_or_else(Utc::now),
        })
    }
}

fn existing_event_id(
    transaction: &mut Transaction<'_>,
    tenant_id: &str,
    event_uuid: &Uuid,
) -> std::result::Result<Option<EventId>, PgOperationError> {
    let existing = transaction.query_opt(
        "SELECT event_id FROM events WHERE tenant_id = $1 AND event_uuid = $2",
        &[&tenant_id, &event_uuid],
    )?;

    existing
        .map(|row| i64_to_event_id(row.get(0), "event_id").map_err(PgOperationError::Storage))
        .transpose()
}

fn next_event_id(
    transaction: &mut Transaction<'_>,
    tenant_id: &str,
) -> std::result::Result<EventId, PgOperationError> {
    let latest: Option<i64> = transaction
        .query_one(
            "SELECT MAX(event_id) FROM events WHERE tenant_id = $1",
            &[&tenant_id],
        )?
        .get(0);
    i64_to_event_id(latest.unwrap_or_default(), "latest_event_id")
        .and_then(|latest| {
            latest
                .checked_add(1)
                .ok_or_else(|| storage_error("event_id overflow").into())
        })
        .map_err(PgOperationError::Storage)
}

fn event_from_row(row: &Row) -> Result<Event> {
    let tenant_id_raw: String = row.get("tenant_id");
    let tenant_id = TenantId::new(tenant_id_raw)
        .map_err(|error| storage_error(format!("invalid tenant_id in row: {error}")))?;
    let event_id_raw: i64 = row.get("event_id");
    let event_type_raw: String = row.get("event_type");
    let source_raw: String = row.get("source");
    let causation_event_id_raw: Option<i64> = row.get("causation_event_id");

    Ok(Event {
        tenant_id,
        event_id: Some(i64_to_event_id(event_id_raw, "event_id")?),
        event_uuid: row.get("event_uuid"),
        correlation_id: row.get("correlation_id"),
        causation_event_id: causation_event_id_raw
            .map(|id| i64_to_event_id(id, "causation_event_id"))
            .transpose()?,
        event_type: parse_event_type(&event_type_raw)?,
        actor_id: row.get("actor_id"),
        source: parse_event_source(&source_raw)?,
        source_ref: row.get("source_ref"),
        payload: row.get("payload"),
        ts: Some(row.get("ts")),
    })
}

fn event_id_to_i64(event_id: EventId, label: &str) -> Result<i64> {
    i64::try_from(event_id)
        .map_err(|error| storage_error(format!("{label} out of range: {error}")).into())
}

fn i64_to_event_id(event_id: i64, label: &str) -> Result<EventId> {
    u64::try_from(event_id)
        .map_err(|error| storage_error(format!("invalid {label}: {error}")).into())
}

fn validate_tenant_id(tenant_id: String) -> Result<String> {
    validate_tenant_id_ref(&tenant_id)?;
    Ok(tenant_id)
}

fn validate_tenant_id_ref(tenant_id: &str) -> Result<()> {
    if tenant_id.trim().is_empty() {
        return Err(storage_error("tenant_id is required").into());
    }
    Ok(())
}

/// Set `app.tenant_id` as a transaction-local GUC for RLS policy evaluation.
///
/// Called at the start of every read and write transaction so the RLS policy
/// `tenant_id = current_setting('app.tenant_id', true)` matches only that
/// tenant's rows. Resets automatically on transaction end (safe with pools).
fn set_tenant_local_pg(
    tx: &mut Transaction<'_>,
    tenant_id: &str,
) -> std::result::Result<(), PgOperationError> {
    tx.execute(
        "SELECT set_config('app.tenant_id', $1, true)",
        &[&tenant_id],
    )
    .map(|_| ())
    .map_err(PgOperationError::Postgres)
}

fn pg_op_to_result(error: PgOperationError) -> crate::HivemindError {
    storage_error(format!("{error:?}")).into()
}

#[derive(Debug)]
enum PgOperationError {
    Pool(r2d2::Error),
    Postgres(postgres::Error),
    Storage(crate::HivemindError),
}

impl PgOperationError {
    fn is_transient(&self) -> bool {
        match self {
            Self::Pool(_) => true,
            Self::Postgres(error) => is_transient_postgres_error(error),
            Self::Storage(_) => false,
        }
    }
}

impl fmt::Display for PgOperationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pool(error) => write!(formatter, "{error}"),
            Self::Postgres(error) => write!(formatter, "{error}"),
            Self::Storage(error) => write!(formatter, "{error}"),
        }
    }
}

impl From<postgres::Error> for PgOperationError {
    fn from(error: postgres::Error) -> Self {
        Self::Postgres(error)
    }
}

impl From<crate::HivemindError> for PgOperationError {
    fn from(error: crate::HivemindError) -> Self {
        Self::Storage(error)
    }
}

fn is_transient_postgres_error(error: &postgres::Error) -> bool {
    if let Some(code) = error.code() {
        return matches!(
            *code,
            SqlState::CONNECTION_EXCEPTION
                | SqlState::CONNECTION_DOES_NOT_EXIST
                | SqlState::CONNECTION_FAILURE
                | SqlState::SQLCLIENT_UNABLE_TO_ESTABLISH_SQLCONNECTION
                | SqlState::TRANSACTION_RESOLUTION_UNKNOWN
                | SqlState::T_R_SERIALIZATION_FAILURE
                | SqlState::T_R_DEADLOCK_DETECTED
        );
    }

    error
        .source()
        .and_then(|source| source.downcast_ref::<std::io::Error>())
        .is_some_and(|error| {
            matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::ConnectionRefused
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::Interrupted
                    | std::io::ErrorKind::TimedOut
            )
        })
}

fn event_columns_sql() -> &'static str {
    "tenant_id,
     event_id,
     event_uuid,
     event_type,
     actor_id,
     source,
     source_ref,
     correlation_id,
     causation_event_id,
     payload,
     ts"
}

fn read_events_sql() -> String {
    format!(
        "SELECT {} FROM events
         WHERE tenant_id = $1 AND event_id > $2
         ORDER BY event_id ASC
         LIMIT $3",
        event_columns_sql()
    )
}

fn replay_events_sql() -> String {
    format!(
        "SELECT {} FROM events
         WHERE tenant_id = $1 AND event_id > $2
         ORDER BY event_id ASC",
        event_columns_sql()
    )
}

fn event_type_as_str(event_type: EventType) -> &'static str {
    match event_type {
        EventType::DecisionProposed => "decision.proposed",
        EventType::DecisionRequested => "decision.requested",
        EventType::DecisionAccepted => "decision.accepted",
        EventType::DecisionRejected => "decision.rejected",
        EventType::DecisionSuperseded => "decision.superseded",
        EventType::EvidenceRecorded => "evidence.recorded",
        EventType::HypothesisRecorded => "hypothesis.recorded",
        EventType::RelationAdded => "relation.added",
        EventType::RelationRemoved => "relation.removed",
        EventType::BlockerReported => "blocker.reported",
        EventType::BlockerResolved => "blocker.resolved",
        EventType::NotificationSent => "notification.sent",
        EventType::NotificationAcknowledged => "notification.acknowledged",
        EventType::IngestBatchReceived => "ingest.batch_received",
        EventType::IngestBatchClassified => "ingest.batch_classified",
        EventType::DecisionScored => "decision.scored",
    }
}

fn parse_event_type(value: &str) -> Result<EventType> {
    match value {
        "decision.proposed" => Ok(EventType::DecisionProposed),
        "decision.requested" => Ok(EventType::DecisionRequested),
        "decision.accepted" => Ok(EventType::DecisionAccepted),
        "decision.rejected" => Ok(EventType::DecisionRejected),
        "decision.superseded" => Ok(EventType::DecisionSuperseded),
        "evidence.recorded" => Ok(EventType::EvidenceRecorded),
        "hypothesis.recorded" => Ok(EventType::HypothesisRecorded),
        "relation.added" => Ok(EventType::RelationAdded),
        "relation.removed" => Ok(EventType::RelationRemoved),
        "blocker.reported" => Ok(EventType::BlockerReported),
        "blocker.resolved" => Ok(EventType::BlockerResolved),
        "notification.sent" => Ok(EventType::NotificationSent),
        "notification.acknowledged" => Ok(EventType::NotificationAcknowledged),
        "ingest.batch_received" => Ok(EventType::IngestBatchReceived),
        "ingest.batch_classified" => Ok(EventType::IngestBatchClassified),
        "decision.scored" => Ok(EventType::DecisionScored),
        other => Err(storage_error(format!("unknown event type in row: {other}")).into()),
    }
}

fn parse_event_source(value: &str) -> Result<EventSource> {
    match value {
        "cli" => Ok(EventSource::Cli),
        "agent" => Ok(EventSource::Agent),
        "human" => Ok(EventSource::Human),
        "slack" => Ok(EventSource::Slack),
        "document" => Ok(EventSource::Document),
        "api" => Ok(EventSource::Api),
        other => Err(storage_error(format!("unknown event source in row: {other}")).into()),
    }
}

#[cfg(test)]
mod tests;
