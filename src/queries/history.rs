use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Value};

use crate::error::QueryError;
use crate::events::{
    self, DecisionIdPayload, DecisionProposedPayload, DecisionSupersededPayload, Event, EventId,
    EventPayload, EventSource, EventType, RelationAddedPayload, RelationKind as EventRelationKind,
};
use crate::ledger::EventLedger;
use crate::projector::NodeKind;
use crate::Result;

use super::{DecisionStatus, QueryResponse, MAX_QUERY_RESULTS};

const DEFAULT_HISTORY_LIMIT: usize = 25;
const LEDGER_READ_PAGE_SIZE: usize = 1_000;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct HistoryFilterRequest {
    pub actor_ids: Vec<String>,
    pub sources: Vec<String>,
    pub source_refs: Vec<String>,
    pub topic_keys: Vec<String>,
    pub statuses: Vec<DecisionStatus>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RecentActivityRequest {
    pub filters: HistoryFilterRequest,
    pub limit: usize,
    pub cursor: Option<String>,
}

impl Default for RecentActivityRequest {
    fn default() -> Self {
        Self {
            filters: HistoryFilterRequest::default(),
            limit: DEFAULT_HISTORY_LIMIT,
            cursor: None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChangedSinceRequest {
    pub since_offset: Option<EventId>,
    pub since_timestamp: Option<DateTime<Utc>>,
    pub until_offset: Option<EventId>,
    pub until_timestamp: Option<DateTime<Utc>>,
    pub filters: HistoryFilterRequest,
    pub limit: usize,
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReadOnlyExportRequest {
    pub query: ReadOnlyExportQuery,
    pub format: ReadOnlyExportFormat,
    pub generated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ReadOnlyExportQuery {
    RecentActivity(RecentActivityRequest),
    DecisionsChangedSince(ChangedSinceRequest),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadOnlyExportFormat {
    Json,
    Markdown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadOnlyExportQueryKind {
    RecentActivity,
    DecisionsChangedSince,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryChangeKind {
    NewDecision,
    StatusChange,
    NewEvidence,
    RefutedAssumption,
    Supersession,
    ContextChange,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct HistoryFilters {
    pub actor_ids: Vec<String>,
    pub sources: Vec<String>,
    pub source_refs: Vec<String>,
    pub topic_keys: Vec<String>,
    pub statuses: Vec<DecisionStatus>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AffectedNode {
    pub id: String,
    pub kind: NodeKind,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LedgerRange {
    pub from_offset_exclusive: EventId,
    pub to_offset_inclusive: EventId,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EventCitation {
    pub citation_id: String,
    pub event_id: EventId,
    pub event_uuid: String,
    pub event_type: EventType,
    pub actor_id: String,
    pub source: EventSource,
    pub source_ref: Option<String>,
    pub ts: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ActivityRow {
    pub event_origin: EventId,
    pub event_uuid: String,
    pub event_type: EventType,
    pub change_kind: HistoryChangeKind,
    pub actor_id: String,
    pub source: EventSource,
    pub source_ref: Option<String>,
    pub ts: Option<DateTime<Utc>>,
    pub decision_ids: Vec<String>,
    pub affected_nodes: Vec<AffectedNode>,
    pub citation_id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RecentActivityResults {
    pub filters: HistoryFilters,
    pub limit: usize,
    pub cursor: Option<String>,
    pub next_cursor: Option<String>,
    pub total_matches: usize,
    pub ledger_range: LedgerRange,
    pub items: Vec<ActivityRow>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ChangeBoundary {
    pub offset: EventId,
    pub timestamp: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct BoundaryEventOffsets {
    pub since_timestamp_offset: Option<EventId>,
    pub until_timestamp_offset: Option<EventId>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DecisionChangeRow {
    pub event_origin: EventId,
    pub event_uuid: String,
    pub event_type: EventType,
    pub change_kind: HistoryChangeKind,
    pub actor_id: String,
    pub source: EventSource,
    pub source_ref: Option<String>,
    pub ts: Option<DateTime<Utc>>,
    pub decision_ids: Vec<String>,
    pub affected_nodes: Vec<AffectedNode>,
    pub citation_id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DecisionsChangedSinceResults {
    pub resolved_since: ChangeBoundary,
    pub resolved_until: ChangeBoundary,
    pub boundary_event_offsets: BoundaryEventOffsets,
    pub filters: HistoryFilters,
    pub limit: usize,
    pub cursor: Option<String>,
    pub next_cursor: Option<String>,
    pub total_matches: usize,
    pub ledger_range: LedgerRange,
    pub items: Vec<DecisionChangeRow>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DecisionsAddedSinceFilterRequest {
    pub actor_ids: Vec<String>,
    pub sources: Vec<String>,
    pub source_refs: Vec<String>,
    pub import_run_ids: Vec<String>,
    pub topic_keys: Vec<String>,
    pub statuses: Vec<DecisionStatus>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DecisionsAddedSinceRequest {
    pub since_offset: Option<EventId>,
    pub since_timestamp: Option<DateTime<Utc>>,
    pub until_offset: Option<EventId>,
    pub until_timestamp: Option<DateTime<Utc>>,
    pub filters: DecisionsAddedSinceFilterRequest,
    pub limit: usize,
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct DecisionsAddedSinceFilters {
    pub actor_ids: Vec<String>,
    pub sources: Vec<String>,
    pub source_refs: Vec<String>,
    pub import_run_ids: Vec<String>,
    pub topic_keys: Vec<String>,
    pub statuses: Vec<DecisionStatus>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DecisionEventProvenance {
    pub event_origin: EventId,
    pub event_uuid: String,
    pub event_type: EventType,
    pub actor_id: String,
    pub source: EventSource,
    pub source_ref: Option<String>,
    pub import_run_id: Option<String>,
    pub ts: Option<DateTime<Utc>>,
    pub citation_id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DecisionChange {
    pub change_kind: HistoryChangeKind,
    pub provenance: DecisionEventProvenance,
    pub affected_nodes: Vec<AffectedNode>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AddedDecisionEntry {
    pub decision_id: String,
    pub status: DecisionStatus,
    pub topic_keys: Vec<String>,
    pub creation: DecisionEventProvenance,
    pub option_ids: Vec<String>,
    pub evidence_ids: Vec<String>,
    pub hypothesis_ids: Vec<String>,
    pub changes_in_window: Vec<DecisionChange>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ChangedDecisionEntry {
    pub decision_id: String,
    pub status: DecisionStatus,
    pub topic_keys: Vec<String>,
    pub creation: Option<DecisionEventProvenance>,
    pub option_ids: Vec<String>,
    pub evidence_ids: Vec<String>,
    pub hypothesis_ids: Vec<String>,
    pub changes_in_window: Vec<DecisionChange>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DecisionsAddedSinceResults {
    pub resolved_since: ChangeBoundary,
    pub resolved_until: ChangeBoundary,
    pub boundary_event_offsets: BoundaryEventOffsets,
    pub filters: DecisionsAddedSinceFilters,
    pub limit: usize,
    pub cursor: Option<String>,
    pub next_cursor: Option<String>,
    pub total_matches: usize,
    pub total_added: usize,
    pub total_changed_existing: usize,
    pub ledger_range: LedgerRange,
    pub added_decisions: Vec<AddedDecisionEntry>,
    pub changed_existing_decisions: Vec<ChangedDecisionEntry>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ReadOnlyExport {
    pub query: ReadOnlyExportQueryKind,
    pub format: ReadOnlyExportFormat,
    pub query_params: BTreeMap<String, Value>,
    pub ledger_range: LedgerRange,
    pub generated_at: DateTime<Utc>,
    pub result_count: usize,
    pub truncated: bool,
    pub continuation_cursor: Option<String>,
    pub citation_map: BTreeMap<String, EventCitation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub json: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markdown: Option<String>,
}

pub fn get_recent_activity(
    ledger: &impl EventLedger,
    request: &RecentActivityRequest,
) -> Result<QueryResponse<RecentActivityResults>> {
    let started = Instant::now();
    let events = read_all_events(ledger)?;
    let latest_offset = ledger.latest_offset()?;
    let index = DecisionIndex::from_events(&events)?;
    let filters = normalized_history_filters(&request.filters);
    let limit = normalized_history_limit(request.limit);
    let cursor = normalized_query(request.cursor.as_deref());
    let offset = parse_history_cursor(cursor.as_deref())?;

    let mut rows = Vec::new();
    for event in &events {
        let row = activity_row(event, &index)?;
        if matches_history_filters(
            &row.actor_id,
            row.source,
            row.source_ref.as_deref(),
            &row.decision_ids,
            &filters,
            &index,
        ) {
            rows.push(row);
        }
    }
    rows.sort_by_key(|row| Reverse(row.event_origin));

    let total_matches = rows.len();
    let items: Vec<ActivityRow> = rows.into_iter().skip(offset).take(limit).collect();
    let next_offset = offset.saturating_add(items.len());
    let next_cursor = (next_offset < total_matches).then(|| next_offset.to_string());

    Ok(QueryResponse {
        result_count: items.len(),
        truncated: next_cursor.is_some(),
        latency_ms: started.elapsed().as_millis(),
        data: RecentActivityResults {
            filters,
            limit,
            cursor,
            next_cursor,
            total_matches,
            ledger_range: LedgerRange {
                from_offset_exclusive: 0,
                to_offset_inclusive: latest_offset,
            },
            items,
        },
    })
}

pub fn get_decisions_changed_since(
    ledger: &impl EventLedger,
    request: &ChangedSinceRequest,
) -> Result<QueryResponse<DecisionsChangedSinceResults>> {
    let started = Instant::now();
    let events = read_all_events(ledger)?;
    let latest_offset = ledger.latest_offset()?;
    let window = resolve_change_window(&events, latest_offset, request);
    let index = DecisionIndex::from_events(&events)?;
    let filters = normalized_history_filters(&request.filters);
    let limit = normalized_history_limit(request.limit);
    let cursor = normalized_query(request.cursor.as_deref());
    let offset = parse_history_cursor(cursor.as_deref())?;

    let mut rows = Vec::new();
    if window.until_offset >= window.since_offset {
        for event in &events {
            let event_id = event_id(event)?;
            if event_id <= window.since_offset || event_id > window.until_offset {
                continue;
            }
            let row = decision_change_row(event, &index)?;
            if matches_history_filters(
                &row.actor_id,
                row.source,
                row.source_ref.as_deref(),
                &row.decision_ids,
                &filters,
                &index,
            ) {
                rows.push(row);
            }
        }
    }
    rows.sort_by_key(|row| row.event_origin);

    let total_matches = rows.len();
    let items: Vec<DecisionChangeRow> = rows.into_iter().skip(offset).take(limit).collect();
    let next_offset = offset.saturating_add(items.len());
    let next_cursor = (next_offset < total_matches).then(|| next_offset.to_string());

    Ok(QueryResponse {
        result_count: items.len(),
        truncated: next_cursor.is_some(),
        latency_ms: started.elapsed().as_millis(),
        data: DecisionsChangedSinceResults {
            resolved_since: ChangeBoundary {
                offset: window.since_offset,
                timestamp: request.since_timestamp,
            },
            resolved_until: ChangeBoundary {
                offset: window.until_offset,
                timestamp: request.until_timestamp,
            },
            boundary_event_offsets: window.boundary_event_offsets,
            filters,
            limit,
            cursor,
            next_cursor,
            total_matches,
            ledger_range: LedgerRange {
                from_offset_exclusive: window.since_offset,
                to_offset_inclusive: window.until_offset,
            },
            items,
        },
    })
}

pub fn get_decisions_added_since(
    ledger: &impl EventLedger,
    request: &DecisionsAddedSinceRequest,
) -> Result<QueryResponse<DecisionsAddedSinceResults>> {
    let started = Instant::now();
    let events = read_all_events(ledger)?;
    let latest_offset = ledger.latest_offset()?;
    let window = resolve_added_since_window(&events, latest_offset, request);
    let index = DecisionIndex::from_events(&events)?;
    let filters = normalized_added_since_filters(&request.filters);
    let limit = normalized_history_limit(request.limit);
    let cursor = normalized_query(request.cursor.as_deref());
    let offset = parse_history_cursor(cursor.as_deref())?;

    let mut creation_events: BTreeMap<String, &Event> = BTreeMap::new();
    let mut per_decision_changes: BTreeMap<String, Vec<(EventId, &Event)>> = BTreeMap::new();

    for event in &events {
        let event_id = event_id(event)?;
        let payload = events::validate(event).map_err(query_validation_error)?;
        let in_window = event_id > window.since_offset && event_id <= window.until_offset;
        if matches!(payload, EventPayload::DecisionProposed(_)) {
            for decision_id in decision_ids_for_payload(&payload, &index) {
                creation_events.entry(decision_id).or_insert(event);
            }
        }
        if !in_window {
            continue;
        }
        for decision_id in decision_ids_for_payload(&payload, &index) {
            per_decision_changes
                .entry(decision_id)
                .or_default()
                .push((event_id, event));
        }
    }

    let mut added: Vec<AddedDecisionEntry> = Vec::new();
    let mut changed: Vec<ChangedDecisionEntry> = Vec::new();

    for (decision_id, change_events) in &per_decision_changes {
        let entry = index.decisions.get(decision_id);
        let topic_keys = entry.map(|entry| entry.topic_keys.clone()).unwrap_or_default();
        let status = index
            .status(decision_id)
            .unwrap_or(DecisionStatus::Proposed);
        let option_ids = entry
            .map(|entry| entry.option_ids.iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        let evidence_ids = entry
            .map(|entry| entry.evidence_ids.iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        let hypothesis_ids = entry
            .map(|entry| entry.hypothesis_ids.iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default();

        let creation_event = creation_events.get(decision_id).copied();
        let creation_provenance = creation_event
            .map(decision_event_provenance)
            .transpose()?;
        let creation_in_window = creation_provenance
            .as_ref()
            .is_some_and(|prov| prov.event_origin > window.since_offset && prov.event_origin <= window.until_offset);

        let mut changes = Vec::with_capacity(change_events.len());
        for (event_id_value, event) in change_events {
            if creation_in_window
                && creation_provenance
                    .as_ref()
                    .is_some_and(|prov| prov.event_origin == *event_id_value)
            {
                continue;
            }
            let payload = events::validate(event).map_err(query_validation_error)?;
            let change_kind = change_kind_for_payload(&payload);
            let affected_nodes = affected_nodes_for_event(event, &payload);
            let provenance = decision_event_provenance(event)?;
            changes.push(DecisionChange {
                change_kind,
                provenance,
                affected_nodes,
            });
        }

        let decision_matches_filters = decision_matches_added_since_filters(
            decision_id,
            &topic_keys,
            status,
            &filters,
            creation_provenance.as_ref(),
            &changes,
        );
        if !decision_matches_filters {
            continue;
        }

        if creation_in_window {
            let creation =
                creation_provenance.expect("creation provenance present when in window");
            added.push(AddedDecisionEntry {
                decision_id: decision_id.clone(),
                status,
                topic_keys,
                creation,
                option_ids,
                evidence_ids,
                hypothesis_ids,
                changes_in_window: changes,
            });
        } else {
            changed.push(ChangedDecisionEntry {
                decision_id: decision_id.clone(),
                status,
                topic_keys,
                creation: creation_provenance,
                option_ids,
                evidence_ids,
                hypothesis_ids,
                changes_in_window: changes,
            });
        }
    }

    added.sort_by(|left, right| {
        (left.creation.event_origin, &left.decision_id)
            .cmp(&(right.creation.event_origin, &right.decision_id))
    });
    changed.sort_by(|left, right| {
        let left_origin = changed_primary_origin(&left.changes_in_window);
        let right_origin = changed_primary_origin(&right.changes_in_window);
        (left_origin, &left.decision_id).cmp(&(right_origin, &right.decision_id))
    });

    let total_added = added.len();
    let total_changed = changed.len();
    let total_matches = total_added.saturating_add(total_changed);

    let mut combined: Vec<DiffEntry> = Vec::with_capacity(total_matches);
    for entry in added.into_iter() {
        combined.push(DiffEntry::Added(entry));
    }
    for entry in changed.into_iter() {
        combined.push(DiffEntry::Changed(entry));
    }

    let paginated: Vec<DiffEntry> = combined.into_iter().skip(offset).take(limit).collect();
    let next_offset = offset.saturating_add(paginated.len());
    let next_cursor = (next_offset < total_matches).then(|| next_offset.to_string());

    let mut added_page: Vec<AddedDecisionEntry> = Vec::new();
    let mut changed_page: Vec<ChangedDecisionEntry> = Vec::new();
    for entry in paginated {
        match entry {
            DiffEntry::Added(entry) => added_page.push(entry),
            DiffEntry::Changed(entry) => changed_page.push(entry),
        }
    }

    Ok(QueryResponse {
        result_count: added_page.len() + changed_page.len(),
        truncated: next_cursor.is_some(),
        latency_ms: started.elapsed().as_millis(),
        data: DecisionsAddedSinceResults {
            resolved_since: ChangeBoundary {
                offset: window.since_offset,
                timestamp: request.since_timestamp,
            },
            resolved_until: ChangeBoundary {
                offset: window.until_offset,
                timestamp: request.until_timestamp,
            },
            boundary_event_offsets: window.boundary_event_offsets,
            filters: filters.into_view(),
            limit,
            cursor,
            next_cursor,
            total_matches,
            total_added,
            total_changed_existing: total_changed,
            ledger_range: LedgerRange {
                from_offset_exclusive: window.since_offset,
                to_offset_inclusive: window.until_offset,
            },
            added_decisions: added_page,
            changed_existing_decisions: changed_page,
        },
    })
}

enum DiffEntry {
    Added(AddedDecisionEntry),
    Changed(ChangedDecisionEntry),
}

fn changed_primary_origin(changes: &[DecisionChange]) -> EventId {
    changes
        .iter()
        .map(|change| change.provenance.event_origin)
        .min()
        .unwrap_or_default()
}

#[derive(Clone, Debug, Default)]
struct NormalizedAddedSinceFilters {
    actor_ids: Vec<String>,
    sources: Vec<String>,
    source_refs: Vec<String>,
    import_run_ids: Vec<String>,
    topic_keys: Vec<String>,
    statuses: Vec<DecisionStatus>,
}

impl NormalizedAddedSinceFilters {
    fn into_view(self) -> DecisionsAddedSinceFilters {
        DecisionsAddedSinceFilters {
            actor_ids: self.actor_ids,
            sources: self.sources,
            source_refs: self.source_refs,
            import_run_ids: self.import_run_ids,
            topic_keys: self.topic_keys,
            statuses: self.statuses,
        }
    }
}

fn normalized_added_since_filters(
    request: &DecisionsAddedSinceFilterRequest,
) -> NormalizedAddedSinceFilters {
    NormalizedAddedSinceFilters {
        actor_ids: normalized_string_values(request.actor_ids.clone()),
        sources: normalized_string_values(request.sources.clone()),
        source_refs: normalized_string_values(request.source_refs.clone()),
        import_run_ids: normalized_string_values(request.import_run_ids.clone()),
        topic_keys: normalized_string_values(request.topic_keys.clone()),
        statuses: request
            .statuses
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
    }
}

fn decision_matches_added_since_filters(
    _decision_id: &str,
    topic_keys: &[String],
    status: DecisionStatus,
    filters: &NormalizedAddedSinceFilters,
    creation: Option<&DecisionEventProvenance>,
    changes: &[DecisionChange],
) -> bool {
    if !filters.statuses.is_empty() && !filters.statuses.contains(&status) {
        return false;
    }
    if !filters.topic_keys.is_empty()
        && !filters
            .topic_keys
            .iter()
            .all(|topic| topic_keys.iter().any(|candidate| candidate == topic))
    {
        return false;
    }

    let mut provenances: Vec<&DecisionEventProvenance> = changes
        .iter()
        .map(|change| &change.provenance)
        .collect();
    if let Some(creation) = creation {
        provenances.push(creation);
    }

    if !filters.actor_ids.is_empty()
        && !provenances
            .iter()
            .any(|prov| filters.actor_ids.iter().any(|id| id == &prov.actor_id))
    {
        return false;
    }
    if !filters.sources.is_empty()
        && !provenances.iter().any(|prov| {
            filters
                .sources
                .iter()
                .any(|expected| prov.source.as_str().eq_ignore_ascii_case(expected))
        })
    {
        return false;
    }
    if !filters.source_refs.is_empty()
        && !provenances.iter().any(|prov| {
            prov.source_ref
                .as_deref()
                .is_some_and(|source_ref| {
                    filters
                        .source_refs
                        .iter()
                        .any(|expected| expected == source_ref)
                })
        })
    {
        return false;
    }
    if !filters.import_run_ids.is_empty()
        && !provenances.iter().any(|prov| {
            prov.import_run_id
                .as_deref()
                .is_some_and(|run_id| {
                    filters
                        .import_run_ids
                        .iter()
                        .any(|expected| expected == run_id)
                })
        })
    {
        return false;
    }

    true
}

fn decision_event_provenance(event: &Event) -> Result<DecisionEventProvenance> {
    let event_id = event_id(event)?;
    Ok(DecisionEventProvenance {
        event_origin: event_id,
        event_uuid: event.event_uuid.to_string(),
        event_type: event.event_type,
        actor_id: event.actor_id.clone(),
        source: event.source,
        source_ref: event.source_ref.clone(),
        import_run_id: extract_import_run_id(event.source_ref.as_deref()),
        ts: event.ts,
        citation_id: citation_id(event_id),
    })
}

fn extract_import_run_id(source_ref: Option<&str>) -> Option<String> {
    let raw = source_ref?;
    let value: Value = serde_json::from_str(raw).ok()?;
    value
        .get("import_run_id")
        .and_then(|value| value.as_str())
        .map(str::to_owned)
}

#[derive(Debug)]
struct ResolvedAddedSinceWindow {
    since_offset: EventId,
    until_offset: EventId,
    boundary_event_offsets: BoundaryEventOffsets,
}

fn resolve_added_since_window(
    events: &[Event],
    latest_offset: EventId,
    request: &DecisionsAddedSinceRequest,
) -> ResolvedAddedSinceWindow {
    let since_timestamp_offset = request
        .since_timestamp
        .map(|timestamp| timestamp_boundary_offset(events, timestamp));
    let until_timestamp_offset = request
        .until_timestamp
        .map(|timestamp| timestamp_boundary_offset(events, timestamp));

    let mut since_offset = request.since_offset.unwrap_or_default();
    if let Some(offset) = since_timestamp_offset {
        since_offset = since_offset.max(offset);
    }
    let mut until_offset = request.until_offset.unwrap_or(latest_offset);
    if let Some(offset) = until_timestamp_offset {
        until_offset = until_offset.min(offset);
    }

    ResolvedAddedSinceWindow {
        since_offset,
        until_offset,
        boundary_event_offsets: BoundaryEventOffsets {
            since_timestamp_offset,
            until_timestamp_offset,
        },
    }
}

pub fn export_read_only_summary(
    ledger: &impl EventLedger,
    request: &ReadOnlyExportRequest,
) -> Result<QueryResponse<ReadOnlyExport>> {
    let started = Instant::now();

    let export = match &request.query {
        ReadOnlyExportQuery::RecentActivity(activity_request) => {
            let response = get_recent_activity(ledger, activity_request)?;
            let query_params = recent_activity_query_params(&response.data);
            let citation_map = activity_citation_map(&response.data.items);
            let json_body = serde_json::to_value(&response.data).map_err(json_query_error)?;
            let markdown = render_recent_activity_markdown(&response.data, request.generated_at);
            export_from_parts(
                ReadOnlyExportQueryKind::RecentActivity,
                request.format,
                query_params,
                response.data.ledger_range.clone(),
                request.generated_at,
                response.result_count,
                response.truncated,
                response.data.next_cursor.clone(),
                citation_map,
                json_body,
                markdown,
            )
        }
        ReadOnlyExportQuery::DecisionsChangedSince(changed_request) => {
            let response = get_decisions_changed_since(ledger, changed_request)?;
            let query_params = changed_since_query_params(&response.data);
            let citation_map = change_citation_map(&response.data.items);
            let json_body = serde_json::to_value(&response.data).map_err(json_query_error)?;
            let markdown = render_changed_since_markdown(&response.data, request.generated_at);
            export_from_parts(
                ReadOnlyExportQueryKind::DecisionsChangedSince,
                request.format,
                query_params,
                response.data.ledger_range.clone(),
                request.generated_at,
                response.result_count,
                response.truncated,
                response.data.next_cursor.clone(),
                citation_map,
                json_body,
                markdown,
            )
        }
    };

    Ok(QueryResponse {
        result_count: export.result_count,
        truncated: export.truncated,
        latency_ms: started.elapsed().as_millis(),
        data: export,
    })
}

#[allow(clippy::too_many_arguments)]
fn export_from_parts(
    query: ReadOnlyExportQueryKind,
    format: ReadOnlyExportFormat,
    query_params: BTreeMap<String, Value>,
    ledger_range: LedgerRange,
    generated_at: DateTime<Utc>,
    result_count: usize,
    truncated: bool,
    continuation_cursor: Option<String>,
    citation_map: BTreeMap<String, EventCitation>,
    json_body: Value,
    markdown: String,
) -> ReadOnlyExport {
    let (json, markdown) = match format {
        ReadOnlyExportFormat::Json => (Some(json_body), None),
        ReadOnlyExportFormat::Markdown => (None, Some(markdown)),
    };

    ReadOnlyExport {
        query,
        format,
        query_params,
        ledger_range,
        generated_at,
        result_count,
        truncated,
        continuation_cursor,
        citation_map,
        json,
        markdown,
    }
}

fn read_all_events(ledger: &impl EventLedger) -> Result<Vec<Event>> {
    let mut events = Vec::new();
    let mut offset = 0;

    loop {
        let page = ledger.read(offset, LEDGER_READ_PAGE_SIZE)?;
        let Some(last) = page.last() else {
            break;
        };
        offset = event_id(last)?;
        let page_len = page.len();
        events.extend(page);
        if page_len < LEDGER_READ_PAGE_SIZE {
            break;
        }
    }

    Ok(events)
}

#[derive(Debug)]
struct ResolvedChangeWindow {
    since_offset: EventId,
    until_offset: EventId,
    boundary_event_offsets: BoundaryEventOffsets,
}

fn resolve_change_window(
    events: &[Event],
    latest_offset: EventId,
    request: &ChangedSinceRequest,
) -> ResolvedChangeWindow {
    let since_timestamp_offset = request
        .since_timestamp
        .map(|timestamp| timestamp_boundary_offset(events, timestamp));
    let until_timestamp_offset = request
        .until_timestamp
        .map(|timestamp| timestamp_boundary_offset(events, timestamp));

    let mut since_offset = request.since_offset.unwrap_or_default();
    if let Some(offset) = since_timestamp_offset {
        since_offset = since_offset.max(offset);
    }

    let mut until_offset = request.until_offset.unwrap_or(latest_offset);
    if let Some(offset) = until_timestamp_offset {
        until_offset = until_offset.min(offset);
    }

    ResolvedChangeWindow {
        since_offset,
        until_offset,
        boundary_event_offsets: BoundaryEventOffsets {
            since_timestamp_offset,
            until_timestamp_offset,
        },
    }
}

fn timestamp_boundary_offset(events: &[Event], timestamp: DateTime<Utc>) -> EventId {
    events
        .iter()
        .filter(|event| event.ts.is_some_and(|event_ts| event_ts <= timestamp))
        .filter_map(|event| event.event_id)
        .max()
        .unwrap_or_default()
}

#[derive(Clone, Debug, Default)]
struct DecisionIndex {
    decisions: BTreeMap<String, DecisionIndexEntry>,
    assumed_by_hypothesis: BTreeMap<String, BTreeSet<String>>,
}

#[derive(Clone, Debug, Default)]
struct DecisionIndexEntry {
    topic_keys: Vec<String>,
    accepted: bool,
    rejected: bool,
    superseded_by: BTreeSet<String>,
    option_ids: BTreeSet<String>,
    evidence_ids: BTreeSet<String>,
    hypothesis_ids: BTreeSet<String>,
}

impl DecisionIndex {
    fn from_events(events: &[Event]) -> Result<Self> {
        let mut index = Self::default();
        for event in events {
            match events::validate(event).map_err(query_validation_error)? {
                EventPayload::DecisionProposed(payload) => {
                    let entry = index
                        .decisions
                        .entry(payload.decision_id.clone())
                        .or_default();
                    entry.topic_keys = normalized_string_values(payload.topic_keys);
                    entry.option_ids.extend(payload.option_ids);
                    entry.evidence_ids.extend(payload.evidence_ids);
                    entry
                        .hypothesis_ids
                        .extend(payload.hypothesis_ids.iter().cloned());
                    for hypothesis_id in payload.hypothesis_ids {
                        index
                            .assumed_by_hypothesis
                            .entry(hypothesis_id)
                            .or_default()
                            .insert(payload.decision_id.clone());
                    }
                }
                EventPayload::DecisionRequested(payload) => {
                    if let Some(decision_id) = payload.decision_id {
                        index.decisions.entry(decision_id).or_default().topic_keys =
                            normalized_string_values(payload.topic_keys);
                    }
                }
                EventPayload::BlockerReported(payload) => {
                    if let Some(decision_id) = payload.decision_id {
                        index.decisions.entry(decision_id).or_default().topic_keys =
                            normalized_string_values(payload.topic_keys);
                    }
                }
                EventPayload::DecisionAccepted(payload) => {
                    index
                        .decisions
                        .entry(payload.decision_id)
                        .or_default()
                        .accepted = true;
                }
                EventPayload::DecisionRejected(payload) => {
                    index
                        .decisions
                        .entry(payload.decision_id)
                        .or_default()
                        .rejected = true;
                }
                EventPayload::DecisionSuperseded(payload) => {
                    index
                        .decisions
                        .entry(payload.old_decision_id)
                        .or_default()
                        .superseded_by
                        .insert(payload.new_decision_id);
                }
                EventPayload::RelationAdded(payload) => match payload.relation {
                    EventRelationKind::Assumes => {
                        index
                            .decisions
                            .entry(payload.from_id.clone())
                            .or_default()
                            .hypothesis_ids
                            .insert(payload.to_id.clone());
                        index
                            .assumed_by_hypothesis
                            .entry(payload.to_id)
                            .or_default()
                            .insert(payload.from_id);
                    }
                    EventRelationKind::BasedOn => {
                        index
                            .decisions
                            .entry(payload.from_id)
                            .or_default()
                            .evidence_ids
                            .insert(payload.to_id);
                    }
                    EventRelationKind::HasOption | EventRelationKind::Chose => {
                        index
                            .decisions
                            .entry(payload.from_id)
                            .or_default()
                            .option_ids
                            .insert(payload.to_id);
                    }
                    EventRelationKind::Supports | EventRelationKind::Refutes => {}
                },
                EventPayload::EvidenceRecorded(_)
                | EventPayload::HypothesisRecorded(_)
                | EventPayload::BlockerResolved(_)
                | EventPayload::NotificationSent(_)
                | EventPayload::NotificationAcknowledged(_) => {}
            }
        }
        Ok(index)
    }

    fn status(&self, decision_id: &str) -> Option<DecisionStatus> {
        let entry = self.decisions.get(decision_id)?;
        if !entry.superseded_by.is_empty() {
            Some(DecisionStatus::Superseded)
        } else {
            match (entry.accepted, entry.rejected) {
                (true, true) => Some(DecisionStatus::Contested),
                (true, false) => Some(DecisionStatus::Accepted),
                (false, true) => Some(DecisionStatus::Rejected),
                (false, false) => Some(DecisionStatus::Proposed),
            }
        }
    }

    fn topic_matches(&self, decision_id: &str, topic_keys: &[String]) -> bool {
        topic_keys.is_empty()
            || self.decisions.get(decision_id).is_some_and(|entry| {
                topic_keys
                    .iter()
                    .all(|topic| entry.topic_keys.contains(topic))
            })
    }

    fn status_matches(&self, decision_id: &str, statuses: &[DecisionStatus]) -> bool {
        statuses.is_empty()
            || self
                .status(decision_id)
                .is_some_and(|status| statuses.contains(&status))
    }

    fn decisions_assuming(&self, hypothesis_id: &str) -> Vec<String> {
        self.assumed_by_hypothesis
            .get(hypothesis_id)
            .map(|ids| ids.iter().cloned().collect())
            .unwrap_or_default()
    }
}

fn activity_row(event: &Event, index: &DecisionIndex) -> Result<ActivityRow> {
    let event_id = event_id(event)?;
    let payload = events::validate(event).map_err(query_validation_error)?;
    let change_kind = change_kind_for_payload(&payload);
    let decision_ids = decision_ids_for_payload(&payload, index);
    let affected_nodes = affected_nodes_for_event(event, &payload);
    let citation = citation_for_event(event)?;

    Ok(ActivityRow {
        event_origin: event_id,
        event_uuid: event.event_uuid.to_string(),
        event_type: event.event_type,
        change_kind,
        actor_id: event.actor_id.clone(),
        source: event.source,
        source_ref: event.source_ref.clone(),
        ts: event.ts,
        decision_ids,
        affected_nodes,
        citation_id: citation.citation_id,
    })
}

fn decision_change_row(event: &Event, index: &DecisionIndex) -> Result<DecisionChangeRow> {
    let event_id = event_id(event)?;
    let payload = events::validate(event).map_err(query_validation_error)?;
    let change_kind = change_kind_for_payload(&payload);
    let decision_ids = decision_ids_for_payload(&payload, index);
    let affected_nodes = affected_nodes_for_event(event, &payload);
    let citation = citation_for_event(event)?;

    Ok(DecisionChangeRow {
        event_origin: event_id,
        event_uuid: event.event_uuid.to_string(),
        event_type: event.event_type,
        change_kind,
        actor_id: event.actor_id.clone(),
        source: event.source,
        source_ref: event.source_ref.clone(),
        ts: event.ts,
        decision_ids,
        affected_nodes,
        citation_id: citation.citation_id,
    })
}

fn change_kind_for_payload(payload: &EventPayload) -> HistoryChangeKind {
    match payload {
        EventPayload::DecisionProposed(_) => HistoryChangeKind::NewDecision,
        EventPayload::DecisionAccepted(_) | EventPayload::DecisionRejected(_) => {
            HistoryChangeKind::StatusChange
        }
        EventPayload::DecisionSuperseded(_) => HistoryChangeKind::Supersession,
        EventPayload::EvidenceRecorded(_) => HistoryChangeKind::NewEvidence,
        EventPayload::RelationAdded(payload) => match payload.relation {
            EventRelationKind::BasedOn | EventRelationKind::Supports => {
                HistoryChangeKind::NewEvidence
            }
            EventRelationKind::Refutes => HistoryChangeKind::RefutedAssumption,
            EventRelationKind::HasOption
            | EventRelationKind::Chose
            | EventRelationKind::Assumes => HistoryChangeKind::ContextChange,
        },
        EventPayload::HypothesisRecorded(_)
        | EventPayload::DecisionRequested(_)
        | EventPayload::BlockerReported(_)
        | EventPayload::BlockerResolved(_)
        | EventPayload::NotificationSent(_)
        | EventPayload::NotificationAcknowledged(_) => HistoryChangeKind::ContextChange,
    }
}

fn decision_ids_for_payload(payload: &EventPayload, index: &DecisionIndex) -> Vec<String> {
    let mut ids = BTreeSet::new();
    match payload {
        EventPayload::DecisionProposed(DecisionProposedPayload { decision_id, .. })
        | EventPayload::DecisionAccepted(DecisionIdPayload { decision_id })
        | EventPayload::DecisionRejected(DecisionIdPayload { decision_id }) => {
            ids.insert(decision_id.clone());
        }
        EventPayload::DecisionSuperseded(DecisionSupersededPayload {
            old_decision_id,
            new_decision_id,
        }) => {
            ids.insert(old_decision_id.clone());
            ids.insert(new_decision_id.clone());
        }
        EventPayload::DecisionRequested(payload) => {
            if let Some(decision_id) = &payload.decision_id {
                ids.insert(decision_id.clone());
            }
        }
        EventPayload::RelationAdded(RelationAddedPayload {
            relation,
            from_id,
            to_id,
        }) => match relation {
            EventRelationKind::BasedOn
            | EventRelationKind::HasOption
            | EventRelationKind::Chose
            | EventRelationKind::Assumes => {
                ids.insert(from_id.clone());
            }
            EventRelationKind::Supports => {}
            EventRelationKind::Refutes => {
                ids.extend(index.decisions_assuming(to_id));
            }
        },
        EventPayload::BlockerReported(payload) => {
            if let Some(decision_id) = &payload.decision_id {
                ids.insert(decision_id.clone());
            }
        }
        EventPayload::EvidenceRecorded(_)
        | EventPayload::HypothesisRecorded(_)
        | EventPayload::BlockerResolved(_)
        | EventPayload::NotificationSent(_)
        | EventPayload::NotificationAcknowledged(_) => {}
    }
    ids.into_iter().collect()
}

fn affected_nodes_for_event(event: &Event, payload: &EventPayload) -> Vec<AffectedNode> {
    let mut nodes = BTreeSet::new();
    match payload {
        EventPayload::DecisionProposed(payload) => {
            nodes.insert(affected_node(&payload.decision_id, NodeKind::Decision));
            nodes.extend(
                payload
                    .option_ids
                    .iter()
                    .map(|id| affected_node(id, NodeKind::Option)),
            );
            nodes.extend(
                payload
                    .hypothesis_ids
                    .iter()
                    .map(|id| affected_node(id, NodeKind::Hypothesis)),
            );
            nodes.extend(
                payload
                    .evidence_ids
                    .iter()
                    .map(|id| affected_node(id, NodeKind::Evidence)),
            );
        }
        EventPayload::DecisionAccepted(payload) | EventPayload::DecisionRejected(payload) => {
            nodes.insert(affected_node(&payload.decision_id, NodeKind::Decision));
        }
        EventPayload::DecisionRequested(payload) => {
            nodes.insert(affected_node(
                &event.event_uuid.to_string(),
                NodeKind::DecisionRequest,
            ));
            nodes.insert(affected_node(&payload.requested_by, NodeKind::Actor));
            if let Some(decision_id) = &payload.decision_id {
                nodes.insert(affected_node(decision_id, NodeKind::Decision));
            }
            if let Some(required_owner_id) = &payload.required_owner_id {
                nodes.insert(affected_node(required_owner_id, NodeKind::Actor));
            }
        }
        EventPayload::DecisionSuperseded(payload) => {
            nodes.insert(affected_node(&payload.old_decision_id, NodeKind::Decision));
            nodes.insert(affected_node(&payload.new_decision_id, NodeKind::Decision));
        }
        EventPayload::EvidenceRecorded(payload) => {
            nodes.insert(affected_node(&payload.evidence_id, NodeKind::Evidence));
        }
        EventPayload::HypothesisRecorded(payload) => {
            nodes.insert(affected_node(&payload.hypothesis_id, NodeKind::Hypothesis));
        }
        EventPayload::RelationAdded(payload) => {
            let (from_kind, to_kind) = event_relation_endpoints(payload.relation);
            nodes.insert(affected_node(&payload.from_id, from_kind));
            nodes.insert(affected_node(&payload.to_id, to_kind));
        }
        EventPayload::BlockerReported(payload) => {
            nodes.insert(affected_node(&payload.blocker_id, NodeKind::Blocker));
            nodes.insert(affected_node(&payload.blocked_actor_id, NodeKind::Actor));
            if let Some(decision_id) = &payload.decision_id {
                nodes.insert(affected_node(decision_id, NodeKind::Decision));
            }
            if let Some(required_owner_id) = &payload.required_owner_id {
                nodes.insert(affected_node(required_owner_id, NodeKind::Actor));
            }
        }
        EventPayload::NotificationSent(payload) => {
            nodes.insert(affected_node(
                &event.event_uuid.to_string(),
                NodeKind::Notification,
            ));
            nodes.insert(affected_node(&payload.blocker_id, NodeKind::Blocker));
            nodes.insert(affected_node(&payload.recipient_actor_id, NodeKind::Actor));
        }
        EventPayload::BlockerResolved(payload) => {
            nodes.insert(affected_node(&payload.blocker_id, NodeKind::Blocker));
        }
        EventPayload::NotificationAcknowledged(payload) => {
            nodes.insert(affected_node(
                &payload.notification_id,
                NodeKind::Notification,
            ));
        }
    }
    nodes.into_iter().collect()
}

fn affected_node(id: &str, kind: NodeKind) -> AffectedNode {
    AffectedNode {
        id: id.to_owned(),
        kind,
    }
}

fn event_relation_endpoints(relation: EventRelationKind) -> (NodeKind, NodeKind) {
    match relation {
        EventRelationKind::BasedOn => (NodeKind::Decision, NodeKind::Evidence),
        EventRelationKind::HasOption | EventRelationKind::Chose => {
            (NodeKind::Decision, NodeKind::Option)
        }
        EventRelationKind::Assumes => (NodeKind::Decision, NodeKind::Hypothesis),
        EventRelationKind::Supports | EventRelationKind::Refutes => {
            (NodeKind::Evidence, NodeKind::Hypothesis)
        }
    }
}

impl Ord for AffectedNode {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.kind, &self.id).cmp(&(other.kind, &other.id))
    }
}

impl PartialOrd for AffectedNode {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

fn matches_history_filters(
    actor_id: &str,
    source: EventSource,
    source_ref: Option<&str>,
    decision_ids: &[String],
    filters: &HistoryFilters,
    index: &DecisionIndex,
) -> bool {
    if !filters.actor_ids.is_empty()
        && !filters
            .actor_ids
            .iter()
            .any(|expected| expected == actor_id)
    {
        return false;
    }
    if !filters.sources.is_empty()
        && !filters
            .sources
            .iter()
            .any(|expected| source.as_str().eq_ignore_ascii_case(expected))
    {
        return false;
    }
    if !filters.source_refs.is_empty()
        && !source_ref.is_some_and(|source_ref| {
            filters
                .source_refs
                .iter()
                .any(|expected| expected == source_ref)
        })
    {
        return false;
    }

    if filters.topic_keys.is_empty() && filters.statuses.is_empty() {
        return true;
    }

    decision_ids.iter().any(|decision_id| {
        index.topic_matches(decision_id, &filters.topic_keys)
            && index.status_matches(decision_id, &filters.statuses)
    })
}

fn normalized_history_filters(request: &HistoryFilterRequest) -> HistoryFilters {
    HistoryFilters {
        actor_ids: normalized_string_values(request.actor_ids.clone()),
        sources: normalized_string_values(request.sources.clone()),
        source_refs: normalized_string_values(request.source_refs.clone()),
        topic_keys: normalized_string_values(request.topic_keys.clone()),
        statuses: request
            .statuses
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
    }
}

fn normalized_string_values(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn normalized_history_limit(limit: usize) -> usize {
    if limit == 0 {
        DEFAULT_HISTORY_LIMIT
    } else {
        limit.min(MAX_QUERY_RESULTS)
    }
}

fn normalized_query(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn parse_history_cursor(cursor: Option<&str>) -> Result<usize> {
    match cursor {
        None => Ok(0),
        Some(cursor) => cursor.parse::<usize>().map_err(|error| {
            query_error(format!("cursor must be a non-negative offset: {error}")).into()
        }),
    }
}

fn event_id(event: &Event) -> Result<EventId> {
    event
        .event_id
        .ok_or_else(|| query_error("event_id is required for history queries").into())
}

fn citation_id(event_id: EventId) -> String {
    format!("event:{event_id}")
}

fn citation_for_event(event: &Event) -> Result<EventCitation> {
    let event_id = event_id(event)?;
    Ok(EventCitation {
        citation_id: citation_id(event_id),
        event_id,
        event_uuid: event.event_uuid.to_string(),
        event_type: event.event_type,
        actor_id: event.actor_id.clone(),
        source: event.source,
        source_ref: event.source_ref.clone(),
        ts: event.ts,
    })
}

fn activity_citation_map(rows: &[ActivityRow]) -> BTreeMap<String, EventCitation> {
    rows.iter()
        .map(|row| {
            let citation = EventCitation {
                citation_id: row.citation_id.clone(),
                event_id: row.event_origin,
                event_uuid: row.event_uuid.clone(),
                event_type: row.event_type,
                actor_id: row.actor_id.clone(),
                source: row.source,
                source_ref: row.source_ref.clone(),
                ts: row.ts,
            };
            (citation.citation_id.clone(), citation)
        })
        .collect()
}

fn change_citation_map(rows: &[DecisionChangeRow]) -> BTreeMap<String, EventCitation> {
    rows.iter()
        .map(|row| {
            let citation = EventCitation {
                citation_id: row.citation_id.clone(),
                event_id: row.event_origin,
                event_uuid: row.event_uuid.clone(),
                event_type: row.event_type,
                actor_id: row.actor_id.clone(),
                source: row.source,
                source_ref: row.source_ref.clone(),
                ts: row.ts,
            };
            (citation.citation_id.clone(), citation)
        })
        .collect()
}

fn recent_activity_query_params(results: &RecentActivityResults) -> BTreeMap<String, Value> {
    BTreeMap::from([
        ("cursor".to_owned(), option_json(results.cursor.as_deref())),
        (
            "filters".to_owned(),
            serde_json::to_value(&results.filters).unwrap_or(Value::Null),
        ),
        ("limit".to_owned(), json!(results.limit)),
        ("query".to_owned(), json!("recent_activity")),
    ])
}

fn changed_since_query_params(results: &DecisionsChangedSinceResults) -> BTreeMap<String, Value> {
    BTreeMap::from([
        ("cursor".to_owned(), option_json(results.cursor.as_deref())),
        (
            "filters".to_owned(),
            serde_json::to_value(&results.filters).unwrap_or(Value::Null),
        ),
        ("limit".to_owned(), json!(results.limit)),
        ("query".to_owned(), json!("decisions_changed_since")),
        (
            "since_offset".to_owned(),
            json!(results.resolved_since.offset),
        ),
        (
            "since_timestamp".to_owned(),
            option_json(
                results
                    .resolved_since
                    .timestamp
                    .as_ref()
                    .map(DateTime::<Utc>::to_rfc3339)
                    .as_deref(),
            ),
        ),
        (
            "until_offset".to_owned(),
            json!(results.resolved_until.offset),
        ),
        (
            "until_timestamp".to_owned(),
            option_json(
                results
                    .resolved_until
                    .timestamp
                    .as_ref()
                    .map(DateTime::<Utc>::to_rfc3339)
                    .as_deref(),
            ),
        ),
    ])
}

fn option_json(value: Option<&str>) -> Value {
    value.map_or(Value::Null, |value| json!(value))
}

fn render_recent_activity_markdown(
    results: &RecentActivityResults,
    generated_at: DateTime<Utc>,
) -> String {
    let mut output = render_markdown_header(
        "recent_activity",
        generated_at,
        &results.ledger_range,
        results.items.len(),
        results.next_cursor.as_deref(),
    );

    for row in &results.items {
        output.push_str(&format!(
            "- event {} {:?} {:?} actor={} source={} citation={}\n",
            row.event_origin,
            row.event_type,
            row.change_kind,
            row.actor_id,
            row.source.as_str(),
            row.citation_id
        ));
        if !row.decision_ids.is_empty() {
            output.push_str(&format!("  decisions: {}\n", row.decision_ids.join(", ")));
        }
    }

    output
}

fn render_changed_since_markdown(
    results: &DecisionsChangedSinceResults,
    generated_at: DateTime<Utc>,
) -> String {
    let mut output = render_markdown_header(
        "decisions_changed_since",
        generated_at,
        &results.ledger_range,
        results.items.len(),
        results.next_cursor.as_deref(),
    );
    output.push_str(&format!(
        "Resolved since: offset {}\nResolved until: offset {}\n\n",
        results.resolved_since.offset, results.resolved_until.offset
    ));

    for row in &results.items {
        output.push_str(&format!(
            "- event {} {:?} {:?} actor={} source={} citation={}\n",
            row.event_origin,
            row.event_type,
            row.change_kind,
            row.actor_id,
            row.source.as_str(),
            row.citation_id
        ));
        if !row.decision_ids.is_empty() {
            output.push_str(&format!("  decisions: {}\n", row.decision_ids.join(", ")));
        }
    }

    output
}

fn render_markdown_header(
    query: &str,
    generated_at: DateTime<Utc>,
    ledger_range: &LedgerRange,
    result_count: usize,
    continuation_cursor: Option<&str>,
) -> String {
    format!(
        "# HiveMind Read-Only Summary\n\nQuery: {query}\nGenerated: {}\nLedger range: >{} through {}\nResults: {result_count}\nTruncated: {}\nContinuation cursor: {}\n\n",
        generated_at.to_rfc3339(),
        ledger_range.from_offset_exclusive,
        ledger_range.to_offset_inclusive,
        continuation_cursor.is_some(),
        continuation_cursor.unwrap_or("none"),
    )
}

fn query_validation_error(error: impl std::fmt::Display) -> QueryError {
    query_error(format!("invalid ledger event in history query: {error}"))
}

fn json_query_error(error: serde_json::Error) -> QueryError {
    query_error(format!("json serialization failed: {error}"))
}

fn query_error(error: impl std::fmt::Display) -> QueryError {
    QueryError::Execution(error.to_string())
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use serde_json::json;
    use uuid::Uuid;

    use crate::ledger::InMemoryEventLedger;

    use super::*;

    #[test]
    fn recent_activity_is_bounded_newest_first_and_cited() -> Result<()> {
        let ledger = fixture_ledger()?;

        let response = get_recent_activity(
            &ledger,
            &RecentActivityRequest {
                limit: 2,
                ..RecentActivityRequest::default()
            },
        )?;

        assert_eq!(response.result_count, 2);
        assert!(response.truncated);
        assert_eq!(response.data.next_cursor.as_deref(), Some("2"));
        assert!(
            response.data.items[0].event_origin > response.data.items[1].event_origin,
            "recent activity must be newest first"
        );
        assert_eq!(response.data.items[0].source, EventSource::Slack);
        assert_eq!(
            response.data.items[0].source_ref.as_deref(),
            Some("thread-1")
        );
        assert_eq!(response.data.items[0].citation_id, "event:8");

        Ok(())
    }

    #[test]
    fn changed_since_classifies_decision_history_and_paginates() -> Result<()> {
        let ledger = fixture_ledger()?;

        let response = get_decisions_changed_since(
            &ledger,
            &ChangedSinceRequest {
                since_offset: Some(1),
                limit: 4,
                ..ChangedSinceRequest::default()
            },
        )?;

        assert_eq!(response.result_count, 4);
        assert!(response.truncated);
        assert_eq!(response.data.next_cursor.as_deref(), Some("4"));
        let kinds = response
            .data
            .items
            .iter()
            .map(|row| row.change_kind)
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            vec![
                HistoryChangeKind::ContextChange,
                HistoryChangeKind::NewDecision,
                HistoryChangeKind::StatusChange,
                HistoryChangeKind::NewDecision,
            ]
        );

        let second_page = get_decisions_changed_since(
            &ledger,
            &ChangedSinceRequest {
                since_offset: Some(1),
                limit: 10,
                cursor: Some("4".to_owned()),
                ..ChangedSinceRequest::default()
            },
        )?;
        let kinds = second_page
            .data
            .items
            .iter()
            .map(|row| row.change_kind)
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            vec![
                HistoryChangeKind::NewEvidence,
                HistoryChangeKind::RefutedAssumption,
                HistoryChangeKind::Supersession,
            ]
        );
        let refuted = second_page
            .data
            .items
            .iter()
            .find(|row| row.change_kind == HistoryChangeKind::RefutedAssumption)
            .expect("refuted assumption row");
        assert_eq!(refuted.decision_ids, vec!["decision-a".to_owned()]);
        assert_eq!(refuted.source_ref.as_deref(), Some("thread-1"));

        Ok(())
    }

    #[test]
    fn changed_since_resolves_timestamp_bounds_to_offsets() -> Result<()> {
        let ledger = fixture_ledger()?;

        let response = get_decisions_changed_since(
            &ledger,
            &ChangedSinceRequest {
                since_timestamp: Some(ts(3)),
                until_timestamp: Some(ts(6)),
                limit: 10,
                ..ChangedSinceRequest::default()
            },
        )?;

        assert_eq!(response.data.resolved_since.offset, 3);
        assert_eq!(response.data.resolved_until.offset, 6);
        assert_eq!(
            response.data.boundary_event_offsets.since_timestamp_offset,
            Some(3)
        );
        assert_eq!(
            response.data.boundary_event_offsets.until_timestamp_offset,
            Some(6)
        );
        assert_eq!(
            response
                .data
                .items
                .iter()
                .map(|row| row.event_origin)
                .collect::<Vec<_>>(),
            vec![4, 5, 6]
        );

        Ok(())
    }

    #[test]
    fn changed_since_handles_request_blocker_and_notification_events() -> Result<()> {
        let ledger = InMemoryEventLedger::new();
        let notification_id = Uuid::from_u128(3).to_string();
        for event in [
            event(
                1,
                EventType::DecisionRequested,
                "actor:requester",
                json!({
                    "topic_keys": ["ops"],
                    "decision_id": "decision-a",
                    "reason": "Need an owner decision",
                    "priority": "P1",
                    "required_owner_id": "actor:owner",
                    "authority_class": "operational",
                    "requested_by": "actor:requester",
                    "client_request_id": "request-1"
                }),
            ),
            event(
                2,
                EventType::BlockerReported,
                "actor:blocked",
                json!({
                    "blocker_id": "blocker-1",
                    "blocked_actor_id": "actor:blocked",
                    "decision_id": "decision-a",
                    "topic_keys": ["ops"],
                    "blocked_ref": "task-1",
                    "blocked_ref_type": "bead",
                    "reason": "Needs decision",
                    "priority": "P1",
                    "last_progress_at": "2026-05-19T12:00:00Z",
                    "required_owner_id": "actor:owner"
                }),
            ),
            event(
                3,
                EventType::NotificationSent,
                "actor:notifier",
                json!({
                    "blocker_id": "blocker-1",
                    "recipient_actor_id": "actor:owner",
                    "channel": "gc",
                    "threshold_rule": "p1",
                    "source_event_ids": [2],
                    "dedupe_key": "blocker-1:p1",
                    "sent_at": "2026-05-19T12:00:03Z"
                }),
            ),
            event(
                4,
                EventType::BlockerResolved,
                "actor:owner",
                json!({
                    "blocker_id": "blocker-1",
                    "resolution_event_id": 4,
                    "resolution_reason": "Decision owner responded"
                }),
            ),
            event(
                5,
                EventType::NotificationAcknowledged,
                "actor:owner",
                json!({
                    "notification_id": notification_id,
                    "ack_at": "2026-05-19T12:00:05Z",
                    "snooze_until": null
                }),
            ),
        ] {
            ledger.append(event)?;
        }

        let response = get_decisions_changed_since(
            &ledger,
            &ChangedSinceRequest {
                since_offset: Some(0),
                limit: 10,
                ..ChangedSinceRequest::default()
            },
        )?;

        assert_eq!(response.result_count, 5);
        assert!(response
            .data
            .items
            .iter()
            .all(|row| row.change_kind == HistoryChangeKind::ContextChange));
        assert_eq!(response.data.items[0].decision_ids, vec!["decision-a"]);
        assert_eq!(response.data.items[1].decision_ids, vec!["decision-a"]);
        assert!(response.data.items[2].decision_ids.is_empty());
        assert!(response.data.items[3].decision_ids.is_empty());
        assert!(response.data.items[4].decision_ids.is_empty());
        assert!(response.data.items[0]
            .affected_nodes
            .contains(&affected_node(
                &Uuid::from_u128(1).to_string(),
                NodeKind::DecisionRequest
            )));
        assert!(response.data.items[1]
            .affected_nodes
            .contains(&affected_node("blocker-1", NodeKind::Blocker)));
        assert!(response.data.items[2]
            .affected_nodes
            .contains(&affected_node(
                &Uuid::from_u128(3).to_string(),
                NodeKind::Notification
            )));
        assert!(response.data.items[3]
            .affected_nodes
            .contains(&affected_node("blocker-1", NodeKind::Blocker)));
        assert!(response.data.items[4]
            .affected_nodes
            .contains(&affected_node(
                &Uuid::from_u128(3).to_string(),
                NodeKind::Notification
            )));

        Ok(())
    }

    #[test]
    fn export_summary_includes_query_params_ledger_range_and_citations() -> Result<()> {
        let ledger = fixture_ledger()?;
        let generated_at = Utc.with_ymd_and_hms(2026, 5, 19, 12, 0, 0).unwrap();

        let response = export_read_only_summary(
            &ledger,
            &ReadOnlyExportRequest {
                query: ReadOnlyExportQuery::RecentActivity(RecentActivityRequest {
                    limit: 1,
                    ..RecentActivityRequest::default()
                }),
                format: ReadOnlyExportFormat::Markdown,
                generated_at,
            },
        )?;

        assert_eq!(response.result_count, 1);
        assert!(response.truncated);
        assert_eq!(response.data.generated_at, generated_at);
        assert_eq!(response.data.ledger_range.to_offset_inclusive, 8);
        assert_eq!(response.data.continuation_cursor.as_deref(), Some("1"));
        assert_eq!(
            response.data.query_params.get("query"),
            Some(&json!("recent_activity"))
        );
        assert!(response.data.citation_map.contains_key("event:8"));
        assert!(response.data.json.is_none());
        assert!(response
            .data
            .markdown
            .as_deref()
            .expect("markdown")
            .contains("citation=event:8"));

        Ok(())
    }

    fn fixture_ledger() -> Result<InMemoryEventLedger> {
        let ledger = InMemoryEventLedger::new();
        for (index, event) in [
            event(
                1,
                EventType::EvidenceRecorded,
                "actor:researcher",
                json!({
                    "evidence_id": "evidence-1",
                    "content": "Baseline evidence",
                    "source": "seed"
                }),
            ),
            event(
                2,
                EventType::HypothesisRecorded,
                "actor:researcher",
                json!({
                    "hypothesis_id": "hypothesis-1",
                    "statement": "Queue pressure stays low"
                }),
            ),
            event(
                3,
                EventType::DecisionProposed,
                "actor:planner",
                json!({
                    "decision_id": "decision-a",
                    "title": "Use batch queue",
                    "rationale": "It keeps writes simple",
                    "topic_keys": ["infra"],
                    "option_ids": [],
                    "chosen_option_id": null,
                    "hypothesis_ids": ["hypothesis-1"],
                    "evidence_ids": ["evidence-1"]
                }),
            ),
            event(
                4,
                EventType::DecisionAccepted,
                "actor:reviewer",
                json!({ "decision_id": "decision-a" }),
            ),
            event(
                5,
                EventType::DecisionProposed,
                "actor:planner",
                json!({
                    "decision_id": "decision-b",
                    "title": "Use streaming queue",
                    "rationale": "It reduces latency",
                    "topic_keys": ["infra"],
                    "option_ids": [],
                    "chosen_option_id": null,
                    "hypothesis_ids": [],
                    "evidence_ids": []
                }),
            ),
            event(
                6,
                EventType::RelationAdded,
                "actor:analyst",
                json!({
                    "relation": "BASED_ON",
                    "from_id": "decision-b",
                    "to_id": "evidence-1"
                }),
            ),
            event(
                7,
                EventType::RelationAdded,
                "actor:auditor",
                json!({
                    "relation": "REFUTES",
                    "from_id": "evidence-1",
                    "to_id": "hypothesis-1"
                }),
            ),
            event(
                8,
                EventType::DecisionSuperseded,
                "actor:architect",
                json!({
                    "old_decision_id": "decision-a",
                    "new_decision_id": "decision-b"
                }),
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let mut event = event;
            if index >= 6 {
                event.source = EventSource::Slack;
                event.source_ref = Some("thread-1".to_owned());
            }
            ledger.append(event)?;
        }
        Ok(ledger)
    }

    fn event(sequence: u64, event_type: EventType, actor_id: &str, payload: Value) -> Event {
        Event {
            event_id: None,
            event_uuid: Uuid::from_u128(u128::from(sequence)),
            correlation_id: Some("history-test".to_owned()),
            causation_event_id: None,
            event_type,
            actor_id: actor_id.to_owned(),
            source: EventSource::Api,
            source_ref: Some("history-test".to_owned()),
            payload,
            ts: Some(ts(sequence)),
        }
    }

    fn ts(sequence: u64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 5, 19, 12, 0, 0).unwrap()
            + chrono::Duration::seconds(i64::try_from(sequence).unwrap())
    }

    #[test]
    fn added_since_returns_empty_for_window_before_any_event() -> Result<()> {
        let ledger = fixture_ledger()?;
        let response = get_decisions_added_since(
            &ledger,
            &DecisionsAddedSinceRequest {
                since_timestamp: Some(Utc.with_ymd_and_hms(2026, 5, 18, 0, 0, 0).unwrap()),
                until_timestamp: Some(Utc.with_ymd_and_hms(2026, 5, 19, 11, 0, 0).unwrap()),
                limit: 10,
                ..DecisionsAddedSinceRequest::default()
            },
        )?;

        assert_eq!(response.data.total_added, 0);
        assert_eq!(response.data.total_changed_existing, 0);
        assert!(response.data.added_decisions.is_empty());
        assert!(response.data.changed_existing_decisions.is_empty());
        Ok(())
    }

    #[test]
    fn added_since_separates_added_and_changed_with_deterministic_order() -> Result<()> {
        let ledger = fixture_ledger()?;

        let response = get_decisions_added_since(
            &ledger,
            &DecisionsAddedSinceRequest {
                since_offset: Some(0),
                limit: 50,
                ..DecisionsAddedSinceRequest::default()
            },
        )?;

        assert_eq!(response.data.total_added, 2);
        assert_eq!(response.data.total_changed_existing, 0);
        let added_ids: Vec<_> = response
            .data
            .added_decisions
            .iter()
            .map(|entry| entry.decision_id.clone())
            .collect();
        assert_eq!(added_ids, vec!["decision-a", "decision-b"]);

        let decision_a = &response.data.added_decisions[0];
        assert_eq!(decision_a.creation.event_origin, 3);
        assert_eq!(decision_a.status, DecisionStatus::Superseded);
        assert_eq!(decision_a.evidence_ids, vec!["evidence-1"]);
        assert_eq!(decision_a.hypothesis_ids, vec!["hypothesis-1"]);
        let kinds: Vec<_> = decision_a
            .changes_in_window
            .iter()
            .map(|change| change.change_kind)
            .collect();
        assert!(kinds.contains(&HistoryChangeKind::StatusChange));
        assert!(kinds.contains(&HistoryChangeKind::RefutedAssumption));
        assert!(kinds.contains(&HistoryChangeKind::Supersession));

        let decision_b = &response.data.added_decisions[1];
        assert_eq!(decision_b.creation.event_origin, 5);
        assert!(decision_b
            .changes_in_window
            .iter()
            .any(|change| change.change_kind == HistoryChangeKind::NewEvidence));
        assert_eq!(decision_b.evidence_ids, vec!["evidence-1"]);

        Ok(())
    }

    #[test]
    fn added_since_classifies_changed_existing_when_creation_is_before_window() -> Result<()> {
        let ledger = fixture_ledger()?;

        let response = get_decisions_added_since(
            &ledger,
            &DecisionsAddedSinceRequest {
                since_offset: Some(5),
                limit: 50,
                ..DecisionsAddedSinceRequest::default()
            },
        )?;

        assert_eq!(response.data.total_added, 0);
        assert_eq!(response.data.total_changed_existing, 2);
        let changed_ids: Vec<_> = response
            .data
            .changed_existing_decisions
            .iter()
            .map(|entry| entry.decision_id.clone())
            .collect();
        assert_eq!(changed_ids, vec!["decision-b", "decision-a"]);

        let decision_b_change = &response.data.changed_existing_decisions[0];
        assert!(decision_b_change.creation.is_some());
        assert_eq!(
            decision_b_change.creation.as_ref().unwrap().event_origin,
            5
        );
        let decision_a_change = &response.data.changed_existing_decisions[1];
        assert!(decision_a_change
            .changes_in_window
            .iter()
            .any(|change| change.change_kind == HistoryChangeKind::Supersession));

        Ok(())
    }

    #[test]
    fn added_since_resolves_timestamps_to_offsets_and_reports_boundaries() -> Result<()> {
        let ledger = fixture_ledger()?;

        let response = get_decisions_added_since(
            &ledger,
            &DecisionsAddedSinceRequest {
                since_timestamp: Some(ts(2)),
                until_timestamp: Some(ts(5)),
                limit: 10,
                ..DecisionsAddedSinceRequest::default()
            },
        )?;

        assert_eq!(response.data.resolved_since.offset, 2);
        assert_eq!(response.data.resolved_until.offset, 5);
        assert_eq!(
            response.data.boundary_event_offsets.since_timestamp_offset,
            Some(2)
        );
        assert_eq!(
            response.data.boundary_event_offsets.until_timestamp_offset,
            Some(5)
        );
        assert_eq!(response.data.total_added, 2);
        assert_eq!(response.data.total_changed_existing, 0);
        Ok(())
    }

    #[test]
    fn added_since_paginates_with_continuation_cursor() -> Result<()> {
        let ledger = fixture_ledger()?;

        let first = get_decisions_added_since(
            &ledger,
            &DecisionsAddedSinceRequest {
                since_offset: Some(0),
                limit: 1,
                ..DecisionsAddedSinceRequest::default()
            },
        )?;
        assert_eq!(first.result_count, 1);
        assert!(first.truncated);
        assert_eq!(first.data.next_cursor.as_deref(), Some("1"));
        assert_eq!(first.data.added_decisions.len(), 1);
        assert_eq!(first.data.added_decisions[0].decision_id, "decision-a");

        let second = get_decisions_added_since(
            &ledger,
            &DecisionsAddedSinceRequest {
                since_offset: Some(0),
                limit: 1,
                cursor: Some("1".to_owned()),
                ..DecisionsAddedSinceRequest::default()
            },
        )?;
        assert_eq!(second.result_count, 1);
        assert!(!second.truncated);
        assert!(second.data.next_cursor.is_none());
        assert_eq!(second.data.added_decisions[0].decision_id, "decision-b");

        Ok(())
    }

    #[test]
    fn added_since_filters_by_import_run_id_extracted_from_source_ref() -> Result<()> {
        use crate::events::EventSource;
        use crate::ledger::InMemoryEventLedger;

        let ledger = InMemoryEventLedger::new();
        let import_source_ref = serde_json::json!({
            "source": "document",
            "path": "/docs/notes.md",
            "sha256": "abc",
            "import_run_id": "import:2026-05-19T12:00:00Z:r1",
            "block_id": "blk-1",
            "source_span": {"start_byte": 0, "end_byte": 10},
            "source_snippet": "hi",
            "importer_actor_id": "actor:importer",
            "original_actor_id": null,
            "provisional_actor": true
        })
        .to_string();

        let mut e1 = event(
            1,
            EventType::DecisionProposed,
            "actor:doc",
            json!({
                "decision_id": "decision-doc-1",
                "title": "Doc decision",
                "rationale": "Imported",
                "topic_keys": ["docs"],
                "option_ids": [],
                "chosen_option_id": null,
                "hypothesis_ids": [],
                "evidence_ids": []
            }),
        );
        e1.source = EventSource::Document;
        e1.source_ref = Some(import_source_ref.clone());
        ledger.append(e1)?;

        let mut e2 = event(
            2,
            EventType::DecisionProposed,
            "actor:planner",
            json!({
                "decision_id": "decision-other",
                "title": "Other",
                "rationale": "Plain",
                "topic_keys": ["other"],
                "option_ids": [],
                "chosen_option_id": null,
                "hypothesis_ids": [],
                "evidence_ids": []
            }),
        );
        e2.source = EventSource::Api;
        e2.source_ref = Some("plain-ref".to_owned());
        ledger.append(e2)?;

        let response = get_decisions_added_since(
            &ledger,
            &DecisionsAddedSinceRequest {
                since_offset: Some(0),
                limit: 10,
                filters: DecisionsAddedSinceFilterRequest {
                    import_run_ids: vec!["import:2026-05-19T12:00:00Z:r1".to_owned()],
                    ..DecisionsAddedSinceFilterRequest::default()
                },
                ..DecisionsAddedSinceRequest::default()
            },
        )?;

        assert_eq!(response.data.total_added, 1);
        assert_eq!(
            response.data.added_decisions[0].decision_id,
            "decision-doc-1"
        );
        assert_eq!(
            response.data.added_decisions[0]
                .creation
                .import_run_id
                .as_deref(),
            Some("import:2026-05-19T12:00:00Z:r1")
        );
        Ok(())
    }
}
