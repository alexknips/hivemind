#[cfg(any(test, feature = "shared-backend-postgres"))]
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use clap::Args;
use serde::Serialize;
#[cfg(any(test, feature = "shared-backend-postgres"))]
use uuid::Uuid;

use crate::error::CliError;
#[cfg(any(test, feature = "shared-backend-postgres"))]
use crate::events::{DecisionProposedPayload, EventId, EventType};
use crate::events::{Event, TenantId};
#[cfg(feature = "shared-backend-postgres")]
use crate::ledger::PostgresEventLedger;
use crate::ledger::{EventLedger, SqliteEventLedger};
#[cfg(any(test, feature = "shared-backend-postgres"))]
use crate::projector::{memory::MemoryGraph, rebuild_graph_for_tenant};
#[cfg(any(test, feature = "shared-backend-postgres"))]
use crate::queries::{get_relevant_decisions, DecisionView};
use crate::Result;

const SQLITE_LEDGER_DB_NAME: &str = "ledger.sqlite";

#[derive(Debug, Clone, Args)]
pub struct MigrateArgs {
    /// Source local HiveMind directory, for example sqlite://./hivemind/.
    #[arg(long = "from")]
    pub from: String,

    /// Destination Postgres URL, for example postgres://user:pass@host/db.
    #[arg(long = "to")]
    pub to: String,

    /// Report source events and URI validation without writing to Postgres.
    #[arg(long = "dry-run")]
    pub dry_run: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct MigrationReport {
    pub dry_run: bool,
    pub source: MigrationEndpointReport,
    pub destination: MigrationEndpointReport,
    pub source_events: usize,
    pub would_migrate_events: usize,
    pub migrated_events: usize,
    pub already_present_events: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destination_events_before: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destination_events_after: Option<usize>,
    pub parity: MigrationParityReport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct MigrationEndpointReport {
    pub backend: &'static str,
    pub tenant_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct MigrationParityReport {
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_result_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destination_result_count: Option<usize>,
}

pub(super) fn run_migrate(cli: &super::Cli, args: &MigrateArgs) -> Result<String> {
    let source_dir = sqlite_path_from_uri(&args.from)?;
    let destination_url = postgres_uri(&args.to)?;
    let destination_tenant = super::cli_tenant(cli)?;
    let source_ledger = open_existing_source_sqlite(&source_dir)?;
    let source_tenant = TenantId::local();
    let source_events = collect_events(&source_ledger, &source_tenant)?;
    let source_path = source_dir.display().to_string();

    if args.dry_run {
        let report = dry_run_report(
            &source_events,
            &source_tenant,
            &destination_tenant,
            Some(source_path),
        );
        return format_migration_output(cli.json, &report);
    }

    #[cfg(feature = "shared-backend-postgres")]
    {
        let destination_ledger =
            PostgresEventLedger::connect(destination_url, destination_tenant.as_str())?;
        let report = migrate_ledgers_with_events(
            &source_ledger,
            &source_tenant,
            &source_events,
            Some(source_path),
            &destination_ledger,
            &destination_tenant,
        )?;
        format_migration_output(cli.json, &report)
    }

    #[cfg(not(feature = "shared-backend-postgres"))]
    {
        let _ = destination_url;
        Err(CliError::InvalidInput(
            "migrate to Postgres requires the shared-backend-postgres feature".to_owned(),
        )
        .into())
    }
}

#[cfg(test)]
pub(super) fn migrate_ledgers(
    source: &impl EventLedger,
    source_tenant: &TenantId,
    source_path: Option<String>,
    destination: &impl EventLedger,
    destination_tenant: &TenantId,
) -> Result<MigrationReport> {
    let source_events = collect_events(source, source_tenant)?;
    migrate_ledgers_with_events(
        source,
        source_tenant,
        &source_events,
        source_path,
        destination,
        destination_tenant,
    )
}

#[cfg(any(test, feature = "shared-backend-postgres"))]
fn migrate_ledgers_with_events(
    source: &impl EventLedger,
    source_tenant: &TenantId,
    source_events: &[Event],
    source_path: Option<String>,
    destination: &impl EventLedger,
    destination_tenant: &TenantId,
) -> Result<MigrationReport> {
    let destination_events_before = collect_events(destination, destination_tenant)?;
    let mut destination_by_uuid = destination_event_ids_by_uuid(&destination_events_before)?;
    let mut source_to_destination_event_ids = BTreeMap::new();
    let mut migrated_events = 0;
    let mut already_present_events = 0;

    for source_event in source_events {
        let source_event_id = required_event_id(source_event, "source")?;
        if let Some(destination_event_id) = destination_by_uuid.get(&source_event.event_uuid) {
            source_to_destination_event_ids.insert(source_event_id, *destination_event_id);
            already_present_events += 1;
            continue;
        }

        let mut destination_event = source_event.clone(); // ubs:ignore: migration must copy source event content before rewriting destination scope.
        destination_event.event_id = None;
        destination_event.tenant_id = destination_tenant.clone(); // ubs:ignore: migration rewrites tenant scope while preserving event content.
        if let Some(source_causation_event_id) = source_event.causation_event_id {
            let destination_causation_event_id = source_to_destination_event_ids
                .get(&source_causation_event_id)
                .copied()
                .ok_or_else(|| {
                    CliError::InvalidInput(format!( // ubs:ignore: error-only allocation while reporting a malformed causation chain.
                        "source event {source_event_id} references causation_event_id {source_causation_event_id}, but that event has not been migrated"
                    ))
                })?;
            destination_event.causation_event_id = Some(destination_causation_event_id);
        }

        let destination_event_id =
            destination.append_for_tenant(destination_tenant, destination_event)?;
        destination_by_uuid.insert(source_event.event_uuid, destination_event_id);
        source_to_destination_event_ids.insert(source_event_id, destination_event_id);
        migrated_events += 1;
    }

    let destination_events_after = collect_events(destination, destination_tenant)?;
    let parity = run_parity_check(
        source,
        source_tenant,
        source_events,
        destination,
        destination_tenant,
    )?;

    Ok(MigrationReport {
        dry_run: false,
        source: MigrationEndpointReport {
            backend: "sqlite",
            tenant_id: source_tenant.to_string(),
            path: source_path,
        },
        destination: MigrationEndpointReport {
            backend: "postgres",
            tenant_id: destination_tenant.to_string(),
            path: None,
        },
        source_events: source_events.len(),
        would_migrate_events: source_events.len(),
        migrated_events,
        already_present_events,
        destination_events_before: Some(destination_events_before.len()),
        destination_events_after: Some(destination_events_after.len()),
        parity,
    })
}

fn dry_run_report(
    source_events: &[Event],
    source_tenant: &TenantId,
    destination_tenant: &TenantId,
    source_path: Option<String>,
) -> MigrationReport {
    MigrationReport {
        dry_run: true,
        source: MigrationEndpointReport {
            backend: "sqlite",
            tenant_id: source_tenant.to_string(),
            path: source_path,
        },
        destination: MigrationEndpointReport {
            backend: "postgres",
            tenant_id: destination_tenant.to_string(),
            path: None,
        },
        source_events: source_events.len(),
        would_migrate_events: source_events.len(),
        migrated_events: 0,
        already_present_events: 0,
        destination_events_before: None,
        destination_events_after: None,
        parity: MigrationParityReport {
            status: "skipped",
            reason: Some("dry_run"),
            topic: None,
            source_result_count: None,
            destination_result_count: None,
        },
    }
}

fn collect_events(ledger: &impl EventLedger, tenant_id: &TenantId) -> Result<Vec<Event>> {
    let mut events = Vec::new();
    ledger.replay_from_for_tenant(tenant_id, 0, &mut |event| {
        events.push(event.clone());
        Ok(())
    })?;
    Ok(events)
}

#[cfg(any(test, feature = "shared-backend-postgres"))]
fn destination_event_ids_by_uuid(events: &[Event]) -> Result<BTreeMap<Uuid, EventId>> {
    let mut ids = BTreeMap::new();
    for event in events {
        ids.insert(event.event_uuid, required_event_id(event, "destination")?);
    }
    Ok(ids)
}

#[cfg(any(test, feature = "shared-backend-postgres"))]
fn required_event_id(event: &Event, label: &'static str) -> Result<EventId> {
    event.event_id.ok_or_else(|| {
        CliError::InvalidInput(format!(
            "{label} event {} is missing event_id",
            event.event_uuid
        ))
        .into()
    })
}

#[cfg(any(test, feature = "shared-backend-postgres"))]
fn run_parity_check(
    source: &impl EventLedger,
    source_tenant: &TenantId,
    source_events: &[Event],
    destination: &impl EventLedger,
    destination_tenant: &TenantId,
) -> Result<MigrationParityReport> {
    let Some(topic) = first_decision_topic(source_events)? else {
        return Ok(MigrationParityReport {
            status: "skipped",
            reason: Some("no_decision_topic"),
            topic: None,
            source_result_count: None,
            destination_result_count: None,
        });
    };

    let source_graph = MemoryGraph::default();
    rebuild_graph_for_tenant(source, source_tenant, &source_graph)?;
    let destination_graph = MemoryGraph::default();
    rebuild_graph_for_tenant(destination, destination_tenant, &destination_graph)?;

    let source_response = get_relevant_decisions(&source_graph, &topic, None)?;
    let destination_response = get_relevant_decisions(&destination_graph, &topic, None)?;
    let matched = source_response.result_count == destination_response.result_count
        && source_response.truncated == destination_response.truncated
        && decision_views_match(&source_response.data, &destination_response.data);

    if !matched {
        return Err(CliError::InvalidInput(format!(
            "migration parity check failed for topic {topic}: source_count={} destination_count={}",
            source_response.result_count, destination_response.result_count
        ))
        .into());
    }

    Ok(MigrationParityReport {
        status: "passed",
        reason: None,
        topic: Some(topic),
        source_result_count: Some(source_response.result_count),
        destination_result_count: Some(destination_response.result_count),
    })
}

#[cfg(any(test, feature = "shared-backend-postgres"))]
fn first_decision_topic(events: &[Event]) -> Result<Option<String>> {
    for event in events {
        if event.event_type != EventType::DecisionProposed {
            continue;
        }
        let payload: DecisionProposedPayload =
            serde_json::from_value(event.payload.clone()).map_err(|error| { // ubs:ignore: parity probe parses a cloned JSON payload without mutating ledger events.
                CliError::InvalidInput(format!( // ubs:ignore: error-only allocation for parity parse diagnostics.
                    "decision.proposed payload could not be parsed during migration parity selection: {error}"
                ))
            })?;
        if let Some(topic) = payload
            .topic_keys
            .iter()
            .map(|topic| topic.trim())
            .find(|topic| !topic.is_empty())
        {
            return Ok(Some(topic.to_owned())); // ubs:ignore: topic is retained after iterating borrowed event payload.
        }
    }
    Ok(None)
}

#[cfg(any(test, feature = "shared-backend-postgres"))]
fn decision_views_match(source: &[DecisionView], destination: &[DecisionView]) -> bool {
    source == destination
}

fn format_migration_output(as_json: bool, report: &MigrationReport) -> Result<String> {
    if as_json {
        return super::format_json_value(true, report);
    }

    let mut output = format!(
        "migrate\tfrom={} tenant={} to={} tenant={} dry_run={} source_events={} would_migrate={} migrated={} already_present={}",
        report.source.backend,
        report.source.tenant_id,
        report.destination.backend,
        report.destination.tenant_id,
        report.dry_run,
        report.source_events,
        report.would_migrate_events,
        report.migrated_events,
        report.already_present_events
    );

    if let Some(count) = report.destination_events_before {
        let _ = write!(output, " destination_events_before={count}");
    }
    if let Some(count) = report.destination_events_after {
        let _ = write!(output, " destination_events_after={count}");
    }

    let _ = write!(output, " parity={}", report.parity.status);
    if let Some(reason) = report.parity.reason {
        let _ = write!(output, " parity_reason={reason}");
    }
    if let Some(topic) = &report.parity.topic {
        let _ = write!(output, " parity_topic={topic}");
    }

    Ok(output)
}

fn open_existing_source_sqlite(path: &Path) -> Result<SqliteEventLedger> {
    if !path.exists() {
        return Err(CliError::InvalidInput(format!(
            "source SQLite directory does not exist: {}",
            path.display()
        ))
        .into());
    }

    let ledger_path = path.join(SQLITE_LEDGER_DB_NAME); // ubs:ignore: joined segment is a fixed ledger filename, not caller-controlled.
    if !ledger_path.exists() {
        return Err(CliError::InvalidInput(format!(
            "source SQLite ledger does not exist: {}",
            ledger_path.display()
        ))
        .into());
    }

    SqliteEventLedger::open(path)
}

fn sqlite_path_from_uri(uri: &str) -> Result<PathBuf> {
    let Some(raw_path) = uri.strip_prefix("sqlite://") else {
        return Err(
            CliError::InvalidInput("--from must use sqlite://<hivemind-dir>".to_owned()).into(),
        );
    };
    let path = raw_path.trim();
    if path.is_empty() {
        return Err(
            CliError::InvalidInput("--from sqlite path must not be empty".to_owned()).into(),
        );
    }
    Ok(PathBuf::from(path))
}

fn postgres_uri(uri: &str) -> Result<&str> {
    let uri = uri.trim();
    if uri.starts_with("postgres://") || uri.starts_with("postgresql://") {
        return Ok(uri);
    }
    Err(CliError::InvalidInput("--to must use postgres:// or postgresql://".to_owned()).into())
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use uuid::Uuid;

    use crate::cli::{run, Cli};
    use crate::commands::{CommandContext, Commands, DecisionProposalEventUuids};
    use crate::events::EventProvenance;
    use crate::ledger::InMemoryEventLedger;

    use super::*;

    #[test]
    fn dry_run_reports_source_events_without_destination_writes() -> Result<()> {
        let source = seeded_source_ledger()?;
        let destination_tenant = test_tenant("tenant:acme")?;
        let source_events = collect_events(&source, &TenantId::local())?;

        let report = dry_run_report(
            &source_events,
            &TenantId::local(),
            &destination_tenant,
            Some("./hivemind".to_owned()),
        );

        ensure(report.dry_run, "dry-run report marks dry_run")?;
        ensure_eq(report.source_events, 4, "dry-run source event count")?;
        ensure_eq(
            report.would_migrate_events,
            4,
            "dry-run would migrate count",
        )?;
        ensure_eq(report.migrated_events, 0, "dry-run migrated count")?;
        ensure_eq(
            report.already_present_events,
            0,
            "dry-run already-present count",
        )?;
        ensure_eq(report.parity.status, "skipped", "dry-run parity status")?;
        ensure_eq(
            report.parity.reason,
            Some("dry_run"),
            "dry-run parity reason",
        )?;
        Ok(())
    }

    #[test]
    fn run_migrate_dry_run_reads_existing_sqlite_source() -> Result<()> {
        let source_dir =
            std::env::temp_dir().join(format!("hivemind-migrate-dry-run-{}", Uuid::new_v4()));
        let source = SqliteEventLedger::open(&source_dir)?;
        seed_source_commands(&source)?;
        let from = format!("sqlite://{}", source_dir.display());

        let cli = Cli::parse_from([
            "hivemind",
            "--json",
            "migrate",
            "--from",
            &from,
            "--to",
            "postgres://localhost/hivemind",
            "--tenant",
            "tenant:acme",
            "--dry-run",
        ]);

        let output = run(&cli)?;
        let report: serde_json::Value = serde_json::from_str(&output).map_err(|error| {
            CliError::InvalidInput(format!("dry-run report JSON did not parse: {error}"))
        })?;
        ensure_json_eq(json_at(&report, "/dry_run")?, true, "json dry_run")?;
        ensure_json_eq(json_at(&report, "/source_events")?, 4, "json source_events")?;
        ensure_json_eq(
            json_at(&report, "/would_migrate_events")?,
            4,
            "json would_migrate_events",
        )?;
        ensure_json_eq(
            json_at(&report, "/migrated_events")?,
            0,
            "json migrated_events",
        )?;
        ensure_json_eq(
            json_at(&report, "/parity/status")?,
            "skipped",
            "json parity status",
        )?;
        ensure_json_eq(
            json_at(&report, "/parity/reason")?,
            "dry_run",
            "json parity reason",
        )?;
        Ok(())
    }

    #[test]
    fn migration_replays_events_rewrites_tenant_and_is_idempotent() -> Result<()> {
        let source = seeded_source_ledger()?;
        let destination = InMemoryEventLedger::new();
        let destination_tenant = test_tenant("tenant:acme")?;
        seed_unrelated_destination_event(&destination, &destination_tenant)?;

        let report = migrate_ledgers(
            &source,
            &TenantId::local(),
            Some("./hivemind".to_owned()),
            &destination,
            &destination_tenant,
        )?;

        ensure(!report.dry_run, "migration report is not dry-run")?;
        ensure_eq(report.source_events, 4, "migration source event count")?;
        ensure_eq(report.migrated_events, 4, "migration migrated count")?;
        ensure_eq(
            report.already_present_events,
            0,
            "migration already-present count",
        )?;
        ensure_eq(
            report.destination_events_before,
            Some(1),
            "destination count before migration",
        )?;
        ensure_eq(
            report.destination_events_after,
            Some(5),
            "destination count after migration",
        )?;
        ensure_eq(report.parity.status, "passed", "migration parity status")?;
        ensure_eq(
            report.parity.topic.as_deref(),
            Some("migration"),
            "migration parity topic",
        )?;

        let source_events = collect_events(&source, &TenantId::local())?;
        let destination_events = collect_events(&destination, &destination_tenant)?;
        let migrated_destination_events = destination_events.get(1..).ok_or_else(|| {
            CliError::InvalidInput("destination events missing migrated range".to_owned())
        })?;
        let source_uuids = source_events
            .iter()
            .map(|event| event.event_uuid)
            .collect::<Vec<_>>();
        let destination_uuids = migrated_destination_events
            .iter()
            .map(|event| event.event_uuid)
            .collect::<Vec<_>>();
        ensure_eq(source_uuids, destination_uuids, "migrated event UUIDs")?;
        ensure(
            migrated_destination_events
                .iter()
                .all(|event| event.tenant_id == destination_tenant),
            "migrated events use destination tenant",
        )?;
        ensure_eq(
            migrated_destination_events
                .get(1)
                .and_then(|event| event.causation_event_id),
            Some(2),
            "HAS_OPTION causation id is rewritten",
        )?;
        ensure_eq(
            migrated_destination_events
                .get(2)
                .and_then(|event| event.causation_event_id),
            Some(2),
            "CHOSE causation id is rewritten",
        )?;

        let rerun = migrate_ledgers(
            &source,
            &TenantId::local(),
            None,
            &destination,
            &destination_tenant,
        )?;
        ensure_eq(rerun.migrated_events, 0, "rerun migrated count")?;
        ensure_eq(
            rerun.already_present_events,
            4,
            "rerun already-present count",
        )?;
        ensure_eq(
            rerun.destination_events_before,
            Some(5),
            "rerun destination count before",
        )?;
        ensure_eq(
            rerun.destination_events_after,
            Some(5),
            "rerun destination count after",
        )?;
        Ok(())
    }

    #[test]
    fn validates_endpoint_uri_schemes() -> Result<()> {
        ensure_eq(
            sqlite_path_from_uri("sqlite://./hivemind")?,
            PathBuf::from("./hivemind"),
            "sqlite uri path",
        )?;
        ensure(
            sqlite_path_from_uri("file://./hivemind").is_err(),
            "non-sqlite source URI rejected",
        )?;
        ensure(
            sqlite_path_from_uri("sqlite://").is_err(),
            "empty sqlite source path rejected",
        )?;
        ensure(
            postgres_uri("postgres://localhost/hivemind").is_ok(),
            "postgres URI accepted",
        )?;
        ensure(
            postgres_uri("postgresql://localhost/hivemind").is_ok(),
            "postgresql URI accepted",
        )?;
        ensure(
            postgres_uri("sqlite://./hivemind").is_err(),
            "non-postgres destination URI rejected",
        )?;
        Ok(())
    }

    #[test]
    #[cfg(feature = "shared-backend-postgres")]
    fn postgres_migration_round_trip_replays_and_is_idempotent() -> Result<()> {
        let Some(database_url) = std::env::var("HIVEMIND_TEST_POSTGRES_URL")
            .ok()
            .filter(|value| !value.trim().is_empty())
        else {
            eprintln!("skipping Postgres migration test; set HIVEMIND_TEST_POSTGRES_URL");
            return Ok(());
        };

        let source_dir =
            std::env::temp_dir().join(format!("hivemind-migration-source-{}", Uuid::new_v4()));
        let source = SqliteEventLedger::open(&source_dir)?;
        seed_source_commands(&source)?;
        let destination_tenant = test_tenant(format!("tenant:migration-test-{}", Uuid::new_v4()))?;
        let destination = PostgresEventLedger::connect_with_pool_size(
            &database_url,
            destination_tenant.as_str(),
            4,
        )?;

        let report = migrate_ledgers(
            &source,
            &TenantId::local(),
            Some(source_dir.display().to_string()),
            &destination,
            &destination_tenant,
        )?;
        ensure_eq(report.migrated_events, 4, "postgres migrated count")?;
        ensure_eq(report.parity.status, "passed", "postgres parity status")?;

        let rerun = migrate_ledgers(
            &source,
            &TenantId::local(),
            None,
            &destination,
            &destination_tenant,
        )?;
        ensure_eq(rerun.migrated_events, 0, "postgres rerun migrated count")?;
        ensure_eq(
            rerun.already_present_events,
            4,
            "postgres rerun already-present count",
        )?;
        Ok(())
    }

    fn test_tenant(value: impl Into<String>) -> Result<TenantId> {
        TenantId::new(value)
            .map_err(|error| CliError::InvalidInput(format!("invalid test tenant: {error}")).into())
    }

    fn ensure(condition: bool, context: &str) -> Result<()> {
        if condition {
            Ok(())
        } else {
            Err(CliError::InvalidInput(context.to_owned()).into())
        }
    }

    fn ensure_eq<T>(actual: T, expected: T, context: &str) -> Result<()>
    where
        T: std::fmt::Debug + PartialEq,
    {
        if actual == expected {
            Ok(())
        } else {
            Err(
                CliError::InvalidInput(format!("{context}: expected {expected:?}, got {actual:?}"))
                    .into(),
            )
        }
    }

    fn json_at<'a>(value: &'a serde_json::Value, pointer: &str) -> Result<&'a serde_json::Value> {
        value
            .pointer(pointer)
            .ok_or_else(|| CliError::InvalidInput(format!("missing JSON field {pointer}")).into())
    }

    fn ensure_json_eq<T>(actual: &serde_json::Value, expected: T, context: &str) -> Result<()>
    where
        T: Into<serde_json::Value> + std::fmt::Debug,
    {
        let expected = expected.into();
        if actual == &expected {
            Ok(())
        } else {
            Err(
                CliError::InvalidInput(format!("{context}: expected {expected:?}, got {actual:?}"))
                    .into(),
            )
        }
    }

    fn seeded_source_ledger() -> Result<InMemoryEventLedger> {
        let source = InMemoryEventLedger::new();
        seed_source_commands(&source)?;
        Ok(source)
    }

    fn seed_source_commands(ledger: &impl EventLedger) -> Result<()> {
        let commands = Commands::new_with_context(
            ledger,
            CommandContext::new(TenantId::local(), EventProvenance::cli()),
        );
        commands.record_option_with_id(
            "actor:alice",
            "option:postgres",
            "Postgres",
            "Remote Postgres tenant",
        )?;
        commands.propose_decision_with_id(
            "actor:alice",
            "decision:remote-migration",
            "Migrate local decisions to Postgres",
            "Remote tenants need the same recoverable decision memory as local ledgers.",
            &["migration".to_owned(), "shared-backend".to_owned()],
            &["option:postgres".to_owned()],
            Some("option:postgres"),
            &[],
            &[],
            DecisionProposalEventUuids {
                proposal: Uuid::from_u128(1),
                has_option: vec![Uuid::from_u128(2)],
                chose: Some(Uuid::from_u128(3)),
                assumes: Vec::new(),
                based_on: Vec::new(),
            },
        )?;
        commands.accept_decision_with_uuid(
            "decision:remote-migration",
            "actor:bob",
            Uuid::from_u128(4),
        )?;
        Ok(())
    }

    fn seed_unrelated_destination_event(
        ledger: &InMemoryEventLedger,
        tenant_id: &TenantId,
    ) -> Result<()> {
        let commands = Commands::new_with_context(
            ledger,
            CommandContext::new(tenant_id.clone(), EventProvenance::cli()),
        );
        commands.record_evidence_with_id(
            "actor:carol",
            "evidence:preexisting",
            "Destination tenant already contains an unrelated audit event.",
            None,
            Uuid::from_u128(100),
        )?;
        Ok(())
    }
}
