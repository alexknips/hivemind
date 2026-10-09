//! CLI side of `hivemind migrate` (hivemind-jawy): move a SQLite ledger into a shared deployment,
//! event for event.
//!
//! Two destinations, one set of rules ([`crate::replay`]): `--to-url` sends the events to a
//! cell's `POST /v1/ledger/replay` with its admin key, so no one needs the database password;
//! `--to` appends straight to a Postgres database. Either way each event keeps its uuid, actor,
//! source and time, a causation link follows its cause by uuid, an event the destination already
//! holds is skipped, and the check at the end is that every source uuid is in the destination.

use std::time::Duration;

use serde_json::Value;

use crate::cli::args::{Cli, MigrateArgs};
use crate::cli::render::{format_json_value, MigrateReport, ParityCheckResult};
use crate::error::CliError;
use crate::events::TenantId;
use crate::ledger::SqliteEventLedger;
use crate::replay::{replay_source, ReplayCounts, ReplayEvent};
use crate::Result;

use super::cli_tenant;

/// Takes one batch of source events (and whether to only count) and answers what the destination
/// did with it.
type BatchSink = dyn FnMut(Vec<ReplayEvent>, bool) -> Result<ReplayCounts>;

/// A cell does not cut a write off, so a slow batch answers when it is done; the client waits a
/// little longer than a slow batch needs.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

pub(super) fn run_migrate(cli: &Cli, args: &MigrateArgs) -> Result<String> {
    let source_dir = match &args.from {
        Some(s) => std::path::PathBuf::from(s.strip_prefix("sqlite://").unwrap_or(s.as_str())),
        None => cli.hivemind_dir.clone(),
    };
    check_destination_flags(args)?;
    let source_tenant = cli_tenant(cli)?;
    let destination_tenant = TenantId::new(args.to_tenant.trim().to_owned())
        .map_err(|error| CliError::InvalidInput(format!("--to-tenant is invalid: {error}")))?;
    let source = SqliteEventLedger::open(&source_dir)?;

    // Read the whole source once without a destination: whatever it holds that cannot be carried
    // (a causation link to nothing, an event with no time) is refused here, before the first
    // event is sent, and an empty source is a wrong --from or --tenant, not a successful move.
    let inspected = replay_source(&source, &source_tenant, true, &mut |batch, _| {
        Ok(ReplayCounts {
            received: batch.len(),
            ..ReplayCounts::default()
        })
    })?;
    if inspected.source_events == 0 {
        return Err(CliError::InvalidInput(format!(
            "nothing to migrate: {} holds no events for tenant `{source_tenant}`; check --from and --tenant",
            source_dir.display()
        ))
        .into());
    }

    let (destination, mut sink) = open_destination(args, &destination_tenant)?;
    let moved = replay_source(&source, &source_tenant, args.dry_run, &mut *sink)?;

    // Parity is not a count: every source uuid must be in the destination. A dry run over the
    // source after the move reports an event as new exactly when its uuid is missing there.
    let parity_check = if args.dry_run {
        None
    } else {
        let verified = replay_source(&source, &source_tenant, true, &mut *sink)?;
        let missing = verified.counts.new_events;
        Some(ParityCheckResult {
            source_event_count: verified.source_events,
            present_in_destination: verified.source_events.saturating_sub(missing),
            missing,
            ok: missing == 0,
        })
    };

    let report = MigrateReport {
        dry_run: args.dry_run,
        source_dir: source_dir.display().to_string(),
        source_tenant: source_tenant.to_string(),
        destination,
        destination_tenant: destination_tenant.to_string(),
        source_event_count: moved.source_events,
        new_events: moved.counts.new_events,
        already_present: moved.counts.already_present,
        parity_check,
    };

    if let Some(parity) = report.parity_check.as_ref().filter(|parity| !parity.ok) {
        return Err(CliError::InvalidInput(format!(
            "parity check failed: {} of {} source events are not in tenant `{}` of {}; a dry run lists the count still to move",
            parity.missing, parity.source_event_count, report.destination_tenant, report.destination
        ))
        .into());
    }

    if cli.json {
        return format_json_value(true, &report);
    }
    let header = if report.dry_run {
        format!(
            "Dry run: {} of {} source events would move ({} already in the destination)",
            report.new_events, report.source_event_count, report.already_present
        )
    } else {
        format!(
            "Migration complete: {} of {} source events moved ({} already in the destination)",
            report.new_events, report.source_event_count, report.already_present
        )
    };
    let mut text = format!(
        "{header}\nSource: {} (tenant: {})\nDestination: {} (tenant: {})",
        report.source_dir, report.source_tenant, report.destination, report.destination_tenant
    );
    if let Some(parity) = &report.parity_check {
        text.push_str(&format!(
            "\nParity check: OK (all {} source event uuids are in the destination)",
            parity.source_event_count
        ));
    }
    Ok(text)
}

/// Refuses a flag combination before the source or the destination is touched.
fn check_destination_flags(args: &MigrateArgs) -> Result<()> {
    if args.to.is_some() && args.admin_key_file.is_some() {
        return Err(CliError::InvalidInput(
            "--admin-key-file is the cell's admin key and goes with --to-url; \
             --to writes with the credential in its own database URL"
                .to_owned(),
        )
        .into());
    }
    Ok(())
}

/// The destination's name for the report, and the sink that sends it batches.
fn open_destination(args: &MigrateArgs, tenant: &TenantId) -> Result<(String, Box<BatchSink>)> {
    match (&args.to_url, &args.to) {
        (Some(url), _) => {
            let key_file = args.admin_key_file.as_ref().ok_or_else(|| {
                CliError::InvalidInput("--to-url needs --admin-key-file".to_owned())
            })?;
            let key = std::fs::read_to_string(key_file).map_err(|error| {
                CliError::InvalidInput(format!(
                    "cannot read --admin-key-file {}: {error}",
                    key_file.display()
                ))
            })?;
            let key = key.trim().to_owned();
            if key.is_empty() {
                return Err(CliError::InvalidInput(format!(
                    "--admin-key-file {} is empty",
                    key_file.display()
                ))
                .into());
            }
            let base = url.trim().trim_end_matches('/').to_owned();
            let cell = CellReplayClient::new(&base, key, tenant)?;
            Ok((
                base,
                Box::new(move |batch, dry_run| cell.send(batch, dry_run)),
            ))
        }
        (None, Some(database_url)) => {
            Ok(("postgres".to_owned(), postgres_sink(database_url, tenant)?))
        }
        (None, None) => Err(CliError::InvalidInput(
            "name a destination: --to-url <cell> or --to <postgres-url>".to_owned(),
        )
        .into()),
    }
}

#[cfg(feature = "shared-backend-postgres")]
fn postgres_sink(database_url: &str, tenant: &TenantId) -> Result<Box<BatchSink>> {
    let ledger = crate::ledger::PostgresEventLedger::connect(database_url, tenant.as_str())?;
    let tenant = tenant.clone();
    Ok(Box::new(move |batch, dry_run| {
        crate::replay::replay_events(&ledger, &tenant, batch, dry_run)
    }))
}

#[cfg(not(feature = "shared-backend-postgres"))]
fn postgres_sink(_database_url: &str, _tenant: &TenantId) -> Result<Box<BatchSink>> {
    Err(CliError::InvalidInput(
        "--to writes straight to Postgres, but this binary was built without the \
         shared-backend-postgres feature; use --to-url with the cell's admin key instead"
            .to_owned(),
    )
    .into())
}

/// Sends batches to a cell's `POST /v1/ledger/replay`.
struct CellReplayClient {
    client: reqwest::blocking::Client,
    endpoint: String,
    admin_key: String,
    tenant: String,
}

impl CellReplayClient {
    fn new(base_url: &str, admin_key: String, tenant: &TenantId) -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|error| {
                CliError::InvalidInput(format!("failed to build HTTP client: {error}"))
            })?;
        Ok(Self {
            client,
            endpoint: format!("{base_url}/v1/ledger/replay"),
            admin_key,
            tenant: tenant.to_string(),
        })
    }

    fn send(&self, batch: Vec<ReplayEvent>, dry_run: bool) -> Result<ReplayCounts> {
        let body = serde_json::json!({
            "tenant_id": self.tenant,
            "dry_run": dry_run,
            "events": batch,
        });
        let response = self
            .client
            .post(&self.endpoint)
            .bearer_auth(&self.admin_key)
            .json(&body)
            .send()
            .map_err(|error| {
                CliError::InvalidInput(format!("request to {} failed: {error}", self.endpoint))
            })?;
        let status = response.status();
        let answer: Value = response.json().map_err(|error| {
            CliError::InvalidInput(format!(
                "{} answered {status} with a body that is not JSON: {error}",
                self.endpoint
            ))
        })?;
        if !status.is_success() {
            let message = answer
                .pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or("request failed");
            return Err(CliError::InvalidInput(format!(
                "{} answered {status}: {message}",
                self.endpoint
            ))
            .into());
        }
        serde_json::from_value(answer).map_err(|error| {
            CliError::InvalidInput(format!(
                "{} answered with counts this client cannot read: {error}",
                self.endpoint
            ))
            .into()
        })
    }
}
