use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;

use crate::events::BlockerPriority;
use crate::projector::{GraphRow, GraphView, NodeKind, RelationKind};
use crate::Result;

use super::shared::{
    decision_node_exists, node_rows, normalized_filter_values, normalized_limit, normalized_query,
    optional_datetime, optional_int, optional_string, optional_string_list, parse_cursor,
    query_error, relation_edges_by_kind, relation_targets, required_datetime, required_string,
};
use super::status::{
    derive_decision_status, derive_hypothesis_status, DecisionStatus, HypothesisStatus,
};
use super::QueryResponse;

const DEFAULT_BLOCKER_STALE_AFTER_SECONDS: i64 = 24 * 60 * 60;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ActiveDecisionBlockersRequest {
    pub filters: DecisionBlockerFilters,
    pub limit: usize,
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct DecisionBlockerFilters {
    pub decision_ids: Vec<String>,
    pub topic_keys: Vec<String>,
    pub required_owner_ids: Vec<String>,
    pub blocked_actor_ids: Vec<String>,
    pub priorities: Vec<BlockerPriority>,
    pub now: Option<DateTime<Utc>>,
    pub stale_after_seconds: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DecisionBlockerResults {
    pub filters: DecisionBlockerFilters,
    pub limit: usize,
    pub cursor: Option<String>,
    pub next_cursor: Option<String>,
    pub total_matches: usize,
    pub items: Vec<DecisionBlockerView>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DecisionBlockerView {
    pub id: String,
    pub decision_id: Option<String>,
    pub decision_status: Option<DecisionStatus>,
    pub topic_keys: Vec<String>,
    pub priority: BlockerPriority,
    pub reason: String,
    pub blocked_actor_id: String,
    pub blocked_ref: String,
    pub blocked_ref_type: String,
    pub required_owner_id: Option<String>,
    pub reported_at: DateTime<Utc>,
    pub last_progress_at: DateTime<Utc>,
    pub stale: bool,
    pub refuted_assumption_ids: Vec<String>,
    pub threshold_rule: String,
    pub notification_state: BlockerNotificationState,
    pub source_event_ids: Vec<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BlockerNotificationState {
    pub state: BlockerNotificationStateKind,
    pub notification_id: Option<String>,
    pub channel: Option<String>,
    pub recipient_actor_id: Option<String>,
    pub threshold_rule: Option<String>,
    pub dedupe_key: Option<String>,
    pub last_sent_at: Option<DateTime<Utc>>,
    pub acknowledged_at: Option<DateTime<Utc>>,
    pub snooze_until: Option<DateTime<Utc>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockerNotificationStateKind {
    None,
    Sent,
    Acknowledged,
    Snoozed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BlockerNotificationCandidatesRequest {
    pub now: DateTime<Utc>,
    pub policy_version: String,
    pub limit: usize,
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BlockerNotificationCandidates {
    pub now: DateTime<Utc>,
    pub policy_version: String,
    pub limit: usize,
    pub cursor: Option<String>,
    pub next_cursor: Option<String>,
    pub total_matches: usize,
    pub items: Vec<BlockerNotificationCandidate>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BlockerNotificationCandidate {
    pub blocker_id: String,
    pub decision_id: Option<String>,
    pub topic_keys: Vec<String>,
    pub priority: BlockerPriority,
    pub reason: String,
    pub recipient_actor_id: String,
    pub channel: String,
    pub threshold_rule: String,
    pub dedupe_key: String,
    pub source_event_ids: Vec<u64>,
    pub notification_state: BlockerNotificationState,
}

pub fn get_active_decision_blockers(
    graph: &impl GraphView,
    request: &ActiveDecisionBlockersRequest,
) -> Result<QueryResponse<DecisionBlockerResults>> {
    let started = Instant::now();
    let filters = normalized_blocker_filters(&request.filters);
    let limit = normalized_limit(request.limit);
    let cursor = normalized_query(request.cursor.as_deref());
    let offset = parse_cursor(cursor.as_deref())?;

    let blockers = active_decision_blockers(graph, &filters)?;
    let total_matches = blockers.len();
    let items: Vec<DecisionBlockerView> = blockers.into_iter().skip(offset).take(limit).collect();
    let next_offset = offset.saturating_add(items.len());
    let next_cursor = (next_offset < total_matches).then(|| next_offset.to_string());

    Ok(QueryResponse {
        result_count: items.len(),
        truncated: next_cursor.is_some(),
        latency_ms: started.elapsed().as_millis(),
        data: DecisionBlockerResults {
            filters,
            limit,
            cursor,
            next_cursor,
            total_matches,
            items,
        },
    })
}

pub fn get_blocker_notification_candidates(
    graph: &impl GraphView,
    request: &BlockerNotificationCandidatesRequest,
) -> Result<QueryResponse<BlockerNotificationCandidates>> {
    let started = Instant::now();
    let policy_version = request.policy_version.trim();
    if policy_version.is_empty() {
        return Err(query_error("policy_version must not be empty").into());
    }

    let limit = normalized_limit(request.limit);
    let cursor = normalized_query(request.cursor.as_deref());
    let offset = parse_cursor(cursor.as_deref())?;
    let filters = DecisionBlockerFilters {
        now: Some(request.now),
        ..DecisionBlockerFilters::default()
    };

    let mut candidates = Vec::new();
    for blocker in active_decision_blockers(graph, &filters)? {
        let Some(required_owner_id) = blocker.required_owner_id.as_ref() else {
            continue;
        };
        let rule = threshold_rule_for(blocker.priority);
        let eligible_at = blocker.reported_at + Duration::seconds(rule.initial_delay_seconds);
        if request.now < eligible_at {
            continue;
        }

        if let Some(snooze_until) = blocker.notification_state.snooze_until {
            if request.now < snooze_until {
                continue;
            }
        }

        if let Some(last_sent_at) = blocker.notification_state.last_sent_at {
            let repeat_at = last_sent_at + Duration::seconds(rule.repeat_after_seconds);
            if request.now < repeat_at {
                continue;
            }
        }

        candidates.push(BlockerNotificationCandidate {
            blocker_id: blocker.id.clone(),
            decision_id: blocker.decision_id.clone(),
            topic_keys: blocker.topic_keys.clone(),
            priority: blocker.priority,
            reason: blocker.reason.clone(),
            recipient_actor_id: required_owner_id.clone(),
            channel: rule.channel.to_owned(),
            threshold_rule: rule.name.to_owned(),
            dedupe_key: blocker_dedupe_key(&blocker),
            source_event_ids: blocker.source_event_ids.clone(),
            notification_state: blocker.notification_state.clone(),
        });
    }

    candidates.sort_by(|left, right| {
        (
            left.priority,
            left.decision_id.as_deref().unwrap_or_default(),
            &left.blocker_id,
        )
            .cmp(&(
                right.priority,
                right.decision_id.as_deref().unwrap_or_default(),
                &right.blocker_id,
            ))
    });

    let total_matches = candidates.len();
    let items: Vec<BlockerNotificationCandidate> =
        candidates.into_iter().skip(offset).take(limit).collect();
    let next_offset = offset.saturating_add(items.len());
    let next_cursor = (next_offset < total_matches).then(|| next_offset.to_string());

    Ok(QueryResponse {
        result_count: items.len(),
        truncated: next_cursor.is_some(),
        latency_ms: started.elapsed().as_millis(),
        data: BlockerNotificationCandidates {
            now: request.now,
            policy_version: policy_version.to_owned(),
            limit,
            cursor,
            next_cursor,
            total_matches,
            items,
        },
    })
}

fn active_decision_blockers(
    graph: &impl GraphView,
    filters: &DecisionBlockerFilters,
) -> Result<Vec<DecisionBlockerView>> {
    let blocker_rows = node_rows(graph, NodeKind::Blocker)?;
    let notification_rows = node_rows(graph, NodeKind::Notification)?;
    let edges = relation_edges_by_kind(graph)?;

    let mut blockers = Vec::new();
    for (id, row) in blocker_rows {
        if optional_string(&row, "resolved_at").is_some() {
            continue;
        }

        let decision_id = optional_string(&row, "decision_id");
        if !filters.decision_ids.is_empty()
            && !decision_id
                .as_ref()
                .is_some_and(|id| filters.decision_ids.iter().any(|expected| expected == id))
        {
            continue;
        }

        let topic_keys = optional_string_list(&row, "topic_keys");
        if !filters.topic_keys.is_empty()
            && !filters
                .topic_keys
                .iter()
                .all(|topic| topic_keys.iter().any(|candidate| candidate == topic))
        {
            continue;
        }

        let blocked_actor_id = required_string(&row, "blocked_actor_id")?;
        if !filters.blocked_actor_ids.is_empty()
            && !filters
                .blocked_actor_ids
                .iter()
                .any(|expected| expected == &blocked_actor_id)
        {
            continue;
        }

        let required_owner_id = optional_string(&row, "required_owner_id");
        if !filters.required_owner_ids.is_empty()
            && !filters
                .required_owner_ids
                .iter()
                .any(|expected| required_owner_id.as_deref() == Some(expected.as_str()))
        {
            continue;
        }

        let priority = blocker_priority_from_row(&row)?;
        if !filters.priorities.is_empty() && !filters.priorities.contains(&priority) {
            continue;
        }

        let reported_at = required_datetime(&row, "reported_at")?;
        let last_progress_at = optional_datetime(&row, "last_progress_at")?.unwrap_or(reported_at);
        let stale_after_seconds = filters
            .stale_after_seconds
            .filter(|seconds| *seconds > 0)
            .unwrap_or(DEFAULT_BLOCKER_STALE_AFTER_SECONDS);
        let stale = filters.now.is_some_and(|now| {
            now.signed_duration_since(last_progress_at) >= Duration::seconds(stale_after_seconds)
        });

        let decision_status = if let Some(decision_id) = decision_id.as_deref() {
            if decision_node_exists(graph, decision_id)? {
                Some(derive_decision_status(graph, decision_id)?)
            } else {
                None
            }
        } else {
            None
        };
        let refuted_assumption_ids = if let Some(decision_id) = decision_id.as_deref() {
            refuted_assumption_ids(graph, &edges, decision_id)?
        } else {
            Vec::new()
        };

        blockers.push(DecisionBlockerView {
            id: id.clone(),
            decision_id,
            decision_status,
            topic_keys,
            priority,
            reason: required_string(&row, "reason")?,
            blocked_actor_id,
            blocked_ref: required_string(&row, "blocked_ref")?,
            blocked_ref_type: required_string(&row, "blocked_ref_type")?,
            required_owner_id,
            reported_at,
            last_progress_at,
            stale,
            refuted_assumption_ids,
            threshold_rule: threshold_rule_for(priority).name.to_owned(),
            notification_state: notification_state_for_blocker(
                &notification_rows,
                &id,
                filters.now,
            )?,
            source_event_ids: source_event_ids_for_blocker(&row),
        });
    }

    blockers.sort_by(|left, right| {
        (left.priority, left.reported_at, &left.id).cmp(&(
            right.priority,
            right.reported_at,
            &right.id,
        ))
    });
    Ok(blockers)
}

fn normalized_blocker_filters(filters: &DecisionBlockerFilters) -> DecisionBlockerFilters {
    DecisionBlockerFilters {
        decision_ids: normalized_filter_values(&filters.decision_ids),
        topic_keys: normalized_filter_values(&filters.topic_keys),
        required_owner_ids: normalized_filter_values(&filters.required_owner_ids),
        blocked_actor_ids: normalized_filter_values(&filters.blocked_actor_ids),
        priorities: filters
            .priorities
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        now: filters.now,
        stale_after_seconds: filters.stale_after_seconds,
    }
}

fn refuted_assumption_ids(
    graph: &impl GraphView,
    edges: &BTreeMap<RelationKind, Vec<(String, String)>>,
    decision_id: &str,
) -> Result<Vec<String>> {
    let mut ids = Vec::new();
    for hypothesis_id in relation_targets(edges, &[RelationKind::PremisedOn], decision_id) {
        if derive_hypothesis_status(graph, &hypothesis_id)? == HypothesisStatus::Refuted {
            ids.push(hypothesis_id);
        }
    }
    ids.sort();
    Ok(ids)
}

fn notification_state_for_blocker(
    notification_rows: &BTreeMap<String, GraphRow>,
    blocker_id: &str,
    now: Option<DateTime<Utc>>,
) -> Result<BlockerNotificationState> {
    let mut records = Vec::new();
    for (id, row) in notification_rows {
        if optional_string(row, "blocker_id").as_deref() != Some(blocker_id) {
            continue;
        }

        records.push(NotificationRecord {
            id: id.clone(),
            recipient_actor_id: optional_string(row, "recipient_actor_id"),
            channel: optional_string(row, "channel"),
            threshold_rule: optional_string(row, "threshold_rule"),
            dedupe_key: optional_string(row, "dedupe_key"),
            sent_at: optional_datetime(row, "sent_at")?,
            ack_at: optional_datetime(row, "ack_at")?,
            snooze_until: optional_datetime(row, "snooze_until")?,
        });
    }

    records.sort_by(|left, right| {
        (
            left.sent_at.unwrap_or(DateTime::<Utc>::UNIX_EPOCH),
            &left.id,
        )
            .cmp(&(
                right.sent_at.unwrap_or(DateTime::<Utc>::UNIX_EPOCH),
                &right.id,
            ))
    });

    let Some(record) = records.last() else {
        return Ok(BlockerNotificationState {
            state: BlockerNotificationStateKind::None,
            notification_id: None,
            channel: None,
            recipient_actor_id: None,
            threshold_rule: None,
            dedupe_key: None,
            last_sent_at: None,
            acknowledged_at: None,
            snooze_until: None,
        });
    };

    let state = if record
        .snooze_until
        .is_some_and(|snooze_until| now.is_none_or(|now| now < snooze_until))
    {
        BlockerNotificationStateKind::Snoozed
    } else if record.ack_at.is_some() {
        BlockerNotificationStateKind::Acknowledged
    } else {
        BlockerNotificationStateKind::Sent
    };

    Ok(BlockerNotificationState {
        state,
        notification_id: Some(record.id.clone()),
        channel: record.channel.clone(),
        recipient_actor_id: record.recipient_actor_id.clone(),
        threshold_rule: record.threshold_rule.clone(),
        dedupe_key: record.dedupe_key.clone(),
        last_sent_at: record.sent_at,
        acknowledged_at: record.ack_at,
        snooze_until: record.snooze_until,
    })
}

#[derive(Clone, Debug)]
struct NotificationRecord {
    id: String,
    recipient_actor_id: Option<String>,
    channel: Option<String>,
    threshold_rule: Option<String>,
    dedupe_key: Option<String>,
    sent_at: Option<DateTime<Utc>>,
    ack_at: Option<DateTime<Utc>>,
    snooze_until: Option<DateTime<Utc>>,
}

#[derive(Clone, Copy, Debug)]
struct ThresholdRule {
    name: &'static str,
    channel: &'static str,
    initial_delay_seconds: i64,
    repeat_after_seconds: i64,
}

fn threshold_rule_for(priority: BlockerPriority) -> ThresholdRule {
    match priority {
        BlockerPriority::P0 => ThresholdRule {
            name: "p0_direct_immediate",
            channel: "direct",
            initial_delay_seconds: 0,
            repeat_after_seconds: 15 * 60,
        },
        BlockerPriority::P1 => ThresholdRule {
            name: "p1_direct_15m",
            channel: "direct",
            initial_delay_seconds: 15 * 60,
            repeat_after_seconds: 2 * 60 * 60,
        },
        BlockerPriority::P2 => ThresholdRule {
            name: "p2_queue_immediate",
            channel: "queue",
            initial_delay_seconds: 0,
            repeat_after_seconds: 24 * 60 * 60,
        },
        BlockerPriority::P3 => ThresholdRule {
            name: "p3_digest_2d",
            channel: "digest",
            initial_delay_seconds: 2 * 24 * 60 * 60,
            repeat_after_seconds: 3 * 24 * 60 * 60,
        },
        BlockerPriority::P4 => ThresholdRule {
            name: "p4_digest_2d",
            channel: "digest",
            initial_delay_seconds: 2 * 24 * 60 * 60,
            repeat_after_seconds: 3 * 24 * 60 * 60,
        },
    }
}

fn blocker_dedupe_key(blocker: &DecisionBlockerView) -> String {
    let subject = blocker
        .decision_id
        .as_ref()
        .map(|decision_id| format!("decision:{decision_id}"))
        .unwrap_or_else(|| {
            let topic_key = if blocker.topic_keys.is_empty() {
                "none".to_owned()
            } else {
                blocker.topic_keys.join("+")
            };
            format!("topic:{topic_key}")
        });
    format!(
        "tenant:default|{subject}|state:active|blocked_actor:{}|required_owner:{}|priority:{}",
        blocker.blocked_actor_id,
        blocker.required_owner_id.as_deref().unwrap_or("none"),
        blocker.priority.as_str().to_ascii_lowercase()
    )
}

fn blocker_priority_from_row(row: &GraphRow) -> Result<BlockerPriority> {
    let priority = required_string(row, "priority")?;
    BlockerPriority::parse(&priority)
        .ok_or_else(|| query_error(format!("unknown blocker priority: {priority}")).into())
}

fn source_event_ids_for_blocker(row: &GraphRow) -> Vec<u64> {
    ["reported_event_origin", "event_origin"]
        .into_iter()
        .filter_map(|key| optional_int(row, key))
        .filter_map(|value| u64::try_from(value).ok())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}
