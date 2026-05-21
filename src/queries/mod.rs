use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;

use crate::error::QueryError;
use crate::events::BlockerPriority;
use crate::projector::{GraphParams, GraphRow, GraphValue, GraphView, NodeKind, RelationKind};
use crate::Result;

mod history;

pub use history::*;

const MAX_QUERY_RESULTS: usize = 1000;
const DEFAULT_SEARCH_LIMIT: usize = 25;
const MAX_SNIPPETS_PER_RESULT: usize = 5;
const SNIPPET_MAX_CHARS: usize = 160;
const DEFAULT_BLOCKER_STALE_AFTER_SECONDS: i64 = 24 * 60 * 60;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DecisionStatus {
    Proposed,
    Accepted,
    Rejected,
    Contested,
    Superseded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HypothesisStatus {
    Open,
    Supported,
    Refuted,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct QueryResponse<T> {
    pub result_count: usize,
    pub truncated: bool,
    pub latency_ms: u128,
    pub data: T,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HypothesisContext {
    pub id: String,
    pub status: HypothesisStatus,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DecisionView {
    pub id: String,
    pub title: String,
    pub rationale: String,
    pub topic_keys: Vec<String>,
    pub status: DecisionStatus,
    pub chosen_option_id: Option<String>,
    pub option_ids: Vec<String>,
    pub evidence_ids: Vec<String>,
    pub hypotheses: Vec<HypothesisContext>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SupersessionChain {
    pub decision_ids: Vec<String>,
    pub input_index: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NeighborhoodRoot {
    pub id: String,
    pub kind: NodeKind,
    pub present: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NeighborNode {
    pub id: String,
    pub kind: NodeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision_status: Option<DecisionStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hypothesis_status: Option<HypothesisStatus>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NeighborEdge {
    pub from: String,
    pub to: String,
    pub relation: RelationKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_origin: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NeighborhoodView {
    pub root: NeighborhoodRoot,
    pub nodes: Vec<NeighborNode>,
    pub edges: Vec<NeighborEdge>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchDecisionRequest {
    pub query: Option<String>,
    pub topic_keys: Vec<String>,
    pub statuses: Vec<DecisionStatus>,
    pub actor_ids: Vec<String>,
    pub sources: Vec<String>,
    pub limit: usize,
    pub cursor: Option<String>,
}

impl Default for SearchDecisionRequest {
    fn default() -> Self {
        Self {
            query: None,
            topic_keys: Vec::new(),
            statuses: Vec::new(),
            actor_ids: Vec::new(),
            sources: Vec::new(),
            limit: DEFAULT_SEARCH_LIMIT,
            cursor: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DecisionSearchResults {
    pub query: Option<String>,
    pub filters: SearchDecisionFilters,
    pub limit: usize,
    pub cursor: Option<String>,
    pub next_cursor: Option<String>,
    pub total_matches: usize,
    pub items: Vec<DecisionSearchResult>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct SearchDecisionFilters {
    pub topic_keys: Vec<String>,
    pub statuses: Vec<DecisionStatus>,
    pub actor_ids: Vec<String>,
    pub sources: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DecisionSearchResult {
    pub decision: DecisionView,
    pub rank: u8,
    pub matched_fields: Vec<String>,
    pub snippets: Vec<SearchSnippet>,
    pub graph_context: SearchGraphContext,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SearchSnippet {
    pub field: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SearchGraphContext {
    pub actor_ids: Vec<String>,
    pub supersedes_decision_ids: Vec<String>,
    pub superseded_by_decision_ids: Vec<String>,
    pub option_ids: Vec<String>,
    pub evidence_ids: Vec<String>,
    pub hypotheses: Vec<HypothesisContext>,
    pub matched_nodes: Vec<SearchMatchedNode>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct SearchMatchedNode {
    pub id: String,
    pub kind: NodeKind,
    pub field: String,
}

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

pub fn get_decision(
    graph: &impl GraphView,
    decision_id: &str,
) -> Result<QueryResponse<Option<DecisionView>>> {
    let started = Instant::now();
    let rows = graph.query(
        "MATCH (d:`Decision` {id: $id}) RETURN d.id AS id, d.title AS title, d.rationale AS rationale, d.topic_keys AS topic_keys LIMIT 1;",
        &GraphParams::from([("id".to_owned(), GraphValue::String(decision_id.to_owned()))]),
    )?;

    let data = if let Some(row) = rows.first() {
        let id = required_string(row, "id")?;
        let title = optional_string(row, "title").unwrap_or_default();
        let rationale = optional_string(row, "rationale").unwrap_or_default();
        let topic_keys = optional_string_list(row, "topic_keys");
        let status = derive_decision_status(graph, &id)?;
        let option_ids = neighbor_ids(
            graph,
            &id,
            RelationKind::HasOption,
            NodeKind::Option,
            "option_id",
        )?;
        let chosen_option_id = neighbor_ids(
            graph,
            &id,
            RelationKind::Chose,
            NodeKind::Option,
            "option_id",
        )?
        .into_iter()
        .next();
        let evidence_ids = neighbor_ids(
            graph,
            &id,
            RelationKind::BasedOn,
            NodeKind::Evidence,
            "evidence_id",
        )?;
        let hypothesis_ids = neighbor_ids(
            graph,
            &id,
            RelationKind::Assumes,
            NodeKind::Hypothesis,
            "hypothesis_id",
        )?;
        let mut hypotheses = Vec::with_capacity(hypothesis_ids.len());
        for hypothesis_id in hypothesis_ids {
            hypotheses.push(HypothesisContext {
                status: derive_hypothesis_status(graph, &hypothesis_id)?,
                id: hypothesis_id,
            });
        }

        Some(DecisionView {
            id,
            title,
            rationale,
            topic_keys,
            status,
            chosen_option_id,
            option_ids,
            evidence_ids,
            hypotheses,
        })
    } else {
        None
    };

    Ok(QueryResponse {
        result_count: usize::from(data.is_some()),
        truncated: false,
        latency_ms: started.elapsed().as_millis(),
        data,
    })
}

pub fn get_relevant_decisions(
    graph: &impl GraphView,
    topic: &str,
    status_filter: Option<DecisionStatus>,
) -> Result<QueryResponse<Vec<DecisionView>>> {
    let started = Instant::now();
    let topic = topic.trim();
    if topic.is_empty() {
        return Err(query_error("topic must not be empty").into());
    }

    let normalized_topic = topic.to_owned();
    let count_rows = graph.query(
        "MATCH (d:`Decision`) WHERE $topic IN d.topic_keys RETURN count(d) AS count;",
        &GraphParams::from([(
            "topic".to_owned(),
            GraphValue::String(normalized_topic.clone()),
        )]),
    )?;
    let total_count = read_count(count_rows, "Decision")? as usize;
    let truncated = total_count > MAX_QUERY_RESULTS;

    let decision_rows = graph.query(
        "MATCH (d:`Decision`) WHERE $topic IN d.topic_keys RETURN d.id AS id, d.title AS title, d.rationale AS rationale, d.topic_keys AS topic_keys ORDER BY d.id LIMIT 1000;",
        &GraphParams::from([("topic".to_owned(), GraphValue::String(normalized_topic))]),
    )?;

    let mut decisions = Vec::new();
    for row in decision_rows {
        let id = required_string(&row, "id")?;
        let status = derive_decision_status(graph, &id)?;
        if status_filter.is_some_and(|expected| expected != status) {
            continue;
        }

        decisions.push(DecisionView {
            id,
            title: optional_string(&row, "title").unwrap_or_default(),
            rationale: optional_string(&row, "rationale").unwrap_or_default(),
            topic_keys: optional_string_list(&row, "topic_keys"),
            status,
            chosen_option_id: None,
            option_ids: Vec::new(),
            evidence_ids: Vec::new(),
            hypotheses: Vec::new(),
        });
    }

    Ok(QueryResponse {
        result_count: decisions.len(),
        truncated,
        latency_ms: started.elapsed().as_millis(),
        data: decisions,
    })
}

pub fn search_decisions(
    graph: &impl GraphView,
    request: &SearchDecisionRequest,
) -> Result<QueryResponse<DecisionSearchResults>> {
    let started = Instant::now();
    let query = normalized_query(request.query.as_deref());
    let terms = query_terms(query.as_deref());
    let topic_keys = normalized_filter_values(&request.topic_keys);
    let statuses = normalized_statuses(&request.statuses);
    let actor_ids = normalized_filter_values(&request.actor_ids);
    let sources = normalized_filter_values(&request.sources);
    let limit = normalized_limit(request.limit);
    let cursor = normalized_query(request.cursor.as_deref());
    let offset = parse_cursor(cursor.as_deref())?;

    let decision_rows = node_rows(graph, NodeKind::Decision)?;
    let actor_rows = node_rows(graph, NodeKind::Actor)?;
    let evidence_rows = node_rows(graph, NodeKind::Evidence)?;
    let option_rows = node_rows(graph, NodeKind::Option)?;
    let hypothesis_rows = node_rows(graph, NodeKind::Hypothesis)?;
    let edges = relation_edges_by_kind(graph)?;

    let mut scored = Vec::new();
    for (id, row) in decision_rows {
        let title = optional_string(&row, "title").unwrap_or_default();
        let rationale = optional_string(&row, "rationale").unwrap_or_default();
        let decision_topic_keys = optional_string_list(&row, "topic_keys");
        if !topic_keys.is_empty()
            && !topic_keys.iter().all(|topic| {
                decision_topic_keys
                    .iter()
                    .any(|candidate| candidate == topic)
            })
        {
            continue;
        }

        let status = derive_decision_status(graph, &id)?;
        if !statuses.is_empty() && !statuses.contains(&status) {
            continue;
        }

        let actor_ids_for_decision = relation_targets(
            &edges,
            &[
                RelationKind::ProposedBy,
                RelationKind::AcceptedBy,
                RelationKind::RejectedBy,
            ],
            &id,
        );
        if !actor_ids.is_empty()
            && !actor_ids
                .iter()
                .any(|actor_id| actor_ids_for_decision.iter().any(|id| id == actor_id))
        {
            continue;
        }

        let source = optional_string(&row, "source").unwrap_or_default();
        if !sources.is_empty()
            && !sources
                .iter()
                .any(|expected| source.eq_ignore_ascii_case(expected))
        {
            continue;
        }

        let option_ids = relation_targets(&edges, &[RelationKind::HasOption], &id);
        let chosen_option_id = relation_targets(&edges, &[RelationKind::Chose], &id)
            .into_iter()
            .next();
        let evidence_ids = relation_targets(&edges, &[RelationKind::BasedOn], &id);
        let hypothesis_ids = relation_targets(&edges, &[RelationKind::Assumes], &id);
        let supersedes_decision_ids = relation_targets(&edges, &[RelationKind::Supersedes], &id);
        let superseded_by_decision_ids = relation_sources(&edges, RelationKind::Supersedes, &id);

        let mut hypotheses = Vec::with_capacity(hypothesis_ids.len());
        for hypothesis_id in &hypothesis_ids {
            hypotheses.push(HypothesisContext {
                id: hypothesis_id.clone(),
                status: derive_hypothesis_status(graph, hypothesis_id)?,
            });
        }

        let mut fields = Vec::new();
        fields.push(SearchField::decision("decision.id", &id, 0));
        fields.push(SearchField::decision("decision.title", &title, 1));
        fields.push(SearchField::decision("decision.rationale", &rationale, 2));
        for topic_key in &decision_topic_keys {
            fields.push(SearchField::decision("decision.topic", topic_key, 3));
        }
        fields.push(SearchField::decision(
            "decision.status",
            decision_status_label(status),
            3,
        ));
        if !source.is_empty() {
            fields.push(SearchField::decision("decision.source", &source, 3));
        }
        for actor_id in &actor_ids_for_decision {
            fields.push(SearchField::node(
                "actor.id",
                actor_id,
                3,
                NodeKind::Actor,
                actor_id,
            ));
            if let Some(actor) = actor_rows.get(actor_id) {
                if let Some(source_ref) = optional_string(actor, "source_ref") {
                    fields.push(SearchField::node(
                        "actor.source_ref",
                        &source_ref,
                        3,
                        NodeKind::Actor,
                        actor_id,
                    ));
                }
            }
        }
        for option_id in &option_ids {
            add_node_search_fields(
                &mut fields,
                &option_rows,
                NodeKind::Option,
                option_id,
                &[
                    ("option.id", "id"),
                    ("option.label", "label"),
                    ("option.description", "description"),
                ],
            );
        }
        for evidence_id in &evidence_ids {
            add_node_search_fields(
                &mut fields,
                &evidence_rows,
                NodeKind::Evidence,
                evidence_id,
                &[("evidence.id", "id"), ("evidence.content", "content")],
            );
        }
        for hypothesis_id in &hypothesis_ids {
            add_node_search_fields(
                &mut fields,
                &hypothesis_rows,
                NodeKind::Hypothesis,
                hypothesis_id,
                &[
                    ("hypothesis.id", "id"),
                    ("hypothesis.statement", "statement"),
                ],
            );
        }
        for decision_id in &supersedes_decision_ids {
            fields.push(SearchField::node(
                "supersedes.id",
                decision_id,
                3,
                NodeKind::Decision,
                decision_id,
            ));
        }
        for decision_id in &superseded_by_decision_ids {
            fields.push(SearchField::node(
                "superseded_by.id",
                decision_id,
                3,
                NodeKind::Decision,
                decision_id,
            ));
        }

        let Some(match_info) = evaluate_search_match(query.as_deref(), &terms, &fields) else {
            continue;
        };

        scored.push(ScoredDecisionSearchResult {
            rank: match_info.rank,
            id: id.clone(),
            result: DecisionSearchResult {
                decision: DecisionView {
                    id,
                    title,
                    rationale,
                    topic_keys: decision_topic_keys,
                    status,
                    chosen_option_id,
                    option_ids: option_ids.clone(),
                    evidence_ids: evidence_ids.clone(),
                    hypotheses: hypotheses.clone(),
                },
                rank: match_info.rank,
                matched_fields: match_info.matched_fields,
                snippets: match_info.snippets,
                graph_context: SearchGraphContext {
                    actor_ids: actor_ids_for_decision,
                    supersedes_decision_ids,
                    superseded_by_decision_ids,
                    option_ids,
                    evidence_ids,
                    hypotheses,
                    matched_nodes: match_info.matched_nodes,
                },
            },
        });
    }

    scored.sort_by(|left, right| (left.rank, &left.id).cmp(&(right.rank, &right.id)));

    let total_matches = scored.len();
    let items: Vec<DecisionSearchResult> = scored
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|scored| scored.result)
        .collect();
    let next_offset = offset.saturating_add(items.len());
    let next_cursor = (next_offset < total_matches).then(|| next_offset.to_string());

    Ok(QueryResponse {
        result_count: items.len(),
        truncated: next_cursor.is_some(),
        latency_ms: started.elapsed().as_millis(),
        data: DecisionSearchResults {
            query,
            filters: SearchDecisionFilters {
                topic_keys,
                statuses,
                actor_ids,
                sources,
            },
            limit,
            cursor,
            next_cursor,
            total_matches,
            items,
        },
    })
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

pub fn get_supersession_chain(
    graph: &impl GraphView,
    decision_id: &str,
) -> Result<QueryResponse<SupersessionChain>> {
    let started = query_timer_start();
    if decision_id.trim().is_empty() {
        return Err(query_error("decision_id must not be empty").into());
    }

    let mut visited = BTreeSet::new();
    visited.insert(decision_id.to_owned());

    let mut older = Vec::new();
    let mut cursor = decision_id.to_owned();
    loop {
        let older_neighbors = supersession_neighbors(graph, &cursor, WalkDirection::Older)?;
        let Some(next) = choose_single_neighbor(&cursor, older_neighbors)? else {
            break;
        };
        if !visited.insert(next.clone()) {
            return Err(query_error(format!("cycle detected on edge {cursor} -> {next}")).into());
        }
        older.push(next.clone());
        cursor = next;
    }

    let mut newer = Vec::new();
    let mut cursor = decision_id.to_owned();
    loop {
        let newer_neighbors = supersession_neighbors(graph, &cursor, WalkDirection::Newer)?;
        let Some(next) = choose_single_neighbor(&cursor, newer_neighbors)? else {
            break;
        };
        if !visited.insert(next.clone()) {
            return Err(query_error(format!("cycle detected on edge {next} -> {cursor}")).into());
        }
        newer.push(next.clone());
        cursor = next;
    }

    older.reverse();
    let input_index = older.len();
    let mut decision_ids = older;
    decision_ids.push(decision_id.to_owned());
    decision_ids.extend(newer);

    Ok(QueryResponse {
        result_count: decision_ids.len(),
        truncated: false,
        latency_ms: started.elapsed().as_millis(),
        data: SupersessionChain {
            decision_ids,
            input_index,
        },
    })
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NeighborhoodRequest {
    pub relations: Option<Vec<RelationKind>>,
}

impl NeighborhoodRequest {
    pub fn all() -> Self {
        Self { relations: None }
    }

    pub fn with_relations<I: IntoIterator<Item = RelationKind>>(relations: I) -> Self {
        Self {
            relations: Some(relations.into_iter().collect()),
        }
    }

    fn allows(&self, relation: RelationKind) -> bool {
        match &self.relations {
            None => true,
            Some(allowed) => allowed.contains(&relation),
        }
    }
}

const DECISION_HOP1_RELATIONS: [(RelationKind, NodeKind, Direction); 9] = [
    (
        RelationKind::ProposedBy,
        NodeKind::Actor,
        Direction::Outgoing,
    ),
    (
        RelationKind::AcceptedBy,
        NodeKind::Actor,
        Direction::Outgoing,
    ),
    (
        RelationKind::RejectedBy,
        NodeKind::Actor,
        Direction::Outgoing,
    ),
    (
        RelationKind::HasOption,
        NodeKind::Option,
        Direction::Outgoing,
    ),
    (RelationKind::Chose, NodeKind::Option, Direction::Outgoing),
    (
        RelationKind::BasedOn,
        NodeKind::Evidence,
        Direction::Outgoing,
    ),
    (
        RelationKind::Assumes,
        NodeKind::Hypothesis,
        Direction::Outgoing,
    ),
    (
        RelationKind::Supersedes,
        NodeKind::Decision,
        Direction::Outgoing,
    ),
    (
        RelationKind::Supersedes,
        NodeKind::Decision,
        Direction::Incoming,
    ),
];

const HYPOTHESIS_HOP2_RELATIONS: [(RelationKind, NodeKind, Direction); 2] = [
    (
        RelationKind::Supports,
        NodeKind::Evidence,
        Direction::Incoming,
    ),
    (
        RelationKind::Refutes,
        NodeKind::Evidence,
        Direction::Incoming,
    ),
];

pub fn get_decision_neighborhood(
    graph: &impl GraphView,
    decision_id: &str,
    request: &NeighborhoodRequest,
) -> Result<QueryResponse<NeighborhoodView>> {
    let started = Instant::now();
    let decision_id = decision_id.trim();
    if decision_id.is_empty() {
        return Err(query_error("decision_id must not be empty").into());
    }

    let root_present = decision_node_exists(graph, decision_id)?;
    let root = NeighborhoodRoot {
        id: decision_id.to_owned(),
        kind: NodeKind::Decision,
        present: root_present,
    };

    if !root_present {
        return Ok(QueryResponse {
            result_count: 0,
            truncated: false,
            latency_ms: started.elapsed().as_millis(),
            data: NeighborhoodView {
                root,
                nodes: Vec::new(),
                edges: Vec::new(),
            },
        });
    }

    let mut edges: Vec<NeighborEdge> = Vec::new();
    let mut hypothesis_ids: BTreeSet<String> = BTreeSet::new();

    for (relation, other_kind, direction) in DECISION_HOP1_RELATIONS {
        if !request.allows(relation) {
            continue;
        }
        let pairs = neighbor_pairs(
            graph,
            NodeKind::Decision,
            decision_id,
            relation,
            other_kind,
            direction,
        )?;
        for (other_id, event_origin) in pairs {
            let (from, to) = match direction {
                Direction::Outgoing => (decision_id.to_owned(), other_id.clone()),
                Direction::Incoming => (other_id.clone(), decision_id.to_owned()),
            };
            edges.push(NeighborEdge {
                from,
                to,
                relation,
                event_origin,
            });
            if matches!(other_kind, NodeKind::Hypothesis) {
                hypothesis_ids.insert(other_id);
            }
        }
    }

    for hypothesis_id in &hypothesis_ids {
        for (relation, other_kind, direction) in HYPOTHESIS_HOP2_RELATIONS {
            if !request.allows(relation) {
                continue;
            }
            let pairs = neighbor_pairs(
                graph,
                NodeKind::Hypothesis,
                hypothesis_id,
                relation,
                other_kind,
                direction,
            )?;
            for (other_id, event_origin) in pairs {
                let (from, to) = match direction {
                    Direction::Outgoing => (hypothesis_id.clone(), other_id),
                    Direction::Incoming => (other_id, hypothesis_id.clone()),
                };
                edges.push(NeighborEdge {
                    from,
                    to,
                    relation,
                    event_origin,
                });
            }
        }
    }

    edges.sort_by(|a, b| {
        (a.relation, &a.from, &a.to, a.event_origin).cmp(&(
            b.relation,
            &b.from,
            &b.to,
            b.event_origin,
        ))
    });
    edges.dedup();

    let total_edges = edges.len();
    let truncated = total_edges > MAX_QUERY_RESULTS;
    if truncated {
        edges.truncate(MAX_QUERY_RESULTS);
    }

    let mut node_kinds: BTreeMap<String, NodeKind> = BTreeMap::new();
    node_kinds.insert(decision_id.to_owned(), NodeKind::Decision);

    for (relation, other_kind, direction) in DECISION_HOP1_RELATIONS {
        if !request.allows(relation) {
            continue;
        }
        for edge in &edges {
            if edge.relation != relation {
                continue;
            }
            let other_id = match direction {
                Direction::Outgoing => edge.to.clone(),
                Direction::Incoming => edge.from.clone(),
            };
            node_kinds.entry(other_id).or_insert(other_kind);
        }
    }

    for hypothesis_id in &hypothesis_ids {
        for (relation, other_kind, direction) in HYPOTHESIS_HOP2_RELATIONS {
            if !request.allows(relation) {
                continue;
            }
            for edge in &edges {
                if edge.relation != relation {
                    continue;
                }
                let endpoint = match direction {
                    Direction::Outgoing => &edge.from,
                    Direction::Incoming => &edge.to,
                };
                if endpoint != hypothesis_id {
                    continue;
                }
                let other_id = match direction {
                    Direction::Outgoing => edge.to.clone(),
                    Direction::Incoming => edge.from.clone(),
                };
                node_kinds.entry(other_id).or_insert(other_kind);
            }
        }
    }

    let mut nodes: Vec<NeighborNode> = Vec::with_capacity(node_kinds.len());
    for (id, kind) in node_kinds {
        let decision_status = if matches!(kind, NodeKind::Decision) {
            Some(derive_decision_status(graph, &id)?)
        } else {
            None
        };
        let hypothesis_status = if matches!(kind, NodeKind::Hypothesis) {
            Some(derive_hypothesis_status(graph, &id)?)
        } else {
            None
        };
        nodes.push(NeighborNode {
            id,
            kind,
            decision_status,
            hypothesis_status,
        });
    }
    nodes.sort_by(|a, b| (a.kind, &a.id).cmp(&(b.kind, &b.id)));

    let result_count = nodes.len() + edges.len();

    Ok(QueryResponse {
        result_count,
        truncated,
        latency_ms: started.elapsed().as_millis(),
        data: NeighborhoodView { root, nodes, edges },
    })
}

fn decision_node_exists(graph: &impl GraphView, decision_id: &str) -> Result<bool> {
    let rows = graph.query(
        "MATCH (d:`Decision` {id: $id}) RETURN d.id AS id LIMIT 1;",
        &GraphParams::from([("id".to_owned(), GraphValue::String(decision_id.to_owned()))]),
    )?;
    Ok(!rows.is_empty())
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
    for hypothesis_id in relation_targets(edges, &[RelationKind::Assumes], decision_id) {
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

#[derive(Clone, Debug)]
struct ScoredDecisionSearchResult {
    rank: u8,
    id: String,
    result: DecisionSearchResult,
}

#[derive(Clone, Debug)]
struct SearchField {
    field: String,
    value: String,
    rank: u8,
    node: Option<(NodeKind, String)>,
}

impl SearchField {
    fn decision(field: &str, value: &str, rank: u8) -> Self {
        Self {
            field: field.to_owned(),
            value: value.to_owned(),
            rank,
            node: None,
        }
    }

    fn node(field: &str, value: &str, rank: u8, kind: NodeKind, id: &str) -> Self {
        Self {
            field: field.to_owned(),
            value: value.to_owned(),
            rank,
            node: Some((kind, id.to_owned())),
        }
    }
}

#[derive(Clone, Debug)]
struct SearchMatchInfo {
    rank: u8,
    matched_fields: Vec<String>,
    snippets: Vec<SearchSnippet>,
    matched_nodes: Vec<SearchMatchedNode>,
}

fn normalized_query(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn query_terms(query: Option<&str>) -> Vec<String> {
    query.map_or_else(Vec::new, |query| {
        query
            .split_whitespace()
            .map(|term| term.to_ascii_lowercase())
            .collect()
    })
}

fn normalized_filter_values(values: &[String]) -> Vec<String> {
    values
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn normalized_statuses(values: &[DecisionStatus]) -> Vec<DecisionStatus> {
    values
        .iter()
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn normalized_limit(limit: usize) -> usize {
    if limit == 0 {
        DEFAULT_SEARCH_LIMIT
    } else {
        limit.min(MAX_QUERY_RESULTS)
    }
}

fn parse_cursor(cursor: Option<&str>) -> Result<usize> {
    match cursor {
        None => Ok(0),
        Some(cursor) => cursor.parse::<usize>().map_err(|error| {
            query_error(format!("cursor must be a non-negative offset: {error}")).into()
        }),
    }
}

fn node_rows(graph: &impl GraphView, kind: NodeKind) -> Result<BTreeMap<String, GraphRow>> {
    let table = kind.table_name();
    let cypher = match kind {
        NodeKind::Decision => format!(
            "MATCH (node:`{table}`) RETURN node.id AS id, node.title AS title, node.rationale AS rationale, node.topic_keys AS topic_keys, node.source AS source, node.source_ref AS source_ref, node.event_origin AS event_origin ORDER BY node.id;"
        ),
        NodeKind::DecisionRequest => format!(
            "MATCH (node:`{table}`) RETURN node.id AS id, node.decision_id AS decision_id, node.topic_keys AS topic_keys, node.reason AS reason, node.priority AS priority, node.required_owner_id AS required_owner_id, node.authority_class AS authority_class, node.requested_by AS requested_by, node.client_request_id AS client_request_id, node.source AS source, node.source_ref AS source_ref, node.event_origin AS event_origin ORDER BY node.id;"
        ),
        NodeKind::Actor => format!(
            "MATCH (node:`{table}`) RETURN node.id AS id, node.source AS source, node.source_ref AS source_ref, node.event_origin AS event_origin ORDER BY node.id;"
        ),
        NodeKind::Evidence => format!(
            "MATCH (node:`{table}`) RETURN node.id AS id, node.content AS content, node.source AS source, node.source_ref AS source_ref, node.event_origin AS event_origin ORDER BY node.id;"
        ),
        NodeKind::Option => format!(
            "MATCH (node:`{table}`) RETURN node.id AS id, node.label AS label, node.description AS description, node.source AS source, node.source_ref AS source_ref, node.event_origin AS event_origin ORDER BY node.id;"
        ),
        NodeKind::Hypothesis => format!(
            "MATCH (node:`{table}`) RETURN node.id AS id, node.statement AS statement, node.source AS source, node.source_ref AS source_ref, node.event_origin AS event_origin ORDER BY node.id;"
        ),
        NodeKind::Blocker => format!(
            "MATCH (node:`{table}`) RETURN node.id AS id, node.blocked_actor_id AS blocked_actor_id, node.decision_id AS decision_id, node.topic_keys AS topic_keys, node.blocked_ref AS blocked_ref, node.blocked_ref_type AS blocked_ref_type, node.reason AS reason, node.priority AS priority, node.last_progress_at AS last_progress_at, node.required_owner_id AS required_owner_id, node.reported_at AS reported_at, node.reported_event_origin AS reported_event_origin, node.resolved_at AS resolved_at, node.resolution_event_id AS resolution_event_id, node.resolution_reason AS resolution_reason, node.resolved_event_origin AS resolved_event_origin, node.source AS source, node.source_ref AS source_ref, node.event_origin AS event_origin ORDER BY node.id;"
        ),
        NodeKind::Notification => format!(
            "MATCH (node:`{table}`) RETURN node.id AS id, node.blocker_id AS blocker_id, node.recipient_actor_id AS recipient_actor_id, node.channel AS channel, node.threshold_rule AS threshold_rule, node.source_event_ids AS source_event_ids, node.dedupe_key AS dedupe_key, node.sent_at AS sent_at, node.ack_at AS ack_at, node.snooze_until AS snooze_until, node.source AS source, node.source_ref AS source_ref, node.event_origin AS event_origin ORDER BY node.id;"
        ),
    };

    let mut rows_by_id = BTreeMap::new();
    for mut row in graph.query(&cypher, &GraphParams::new())? {
        let id = required_string(&row, "id")?;
        row.insert("id".to_owned(), GraphValue::String(id.clone()));
        rows_by_id.insert(id, row);
    }
    Ok(rows_by_id)
}

fn relation_edges_by_kind(
    graph: &impl GraphView,
) -> Result<BTreeMap<RelationKind, Vec<(String, String)>>> {
    let mut edges = BTreeMap::new();
    for relation in RelationKind::ALL {
        edges.insert(relation, relation_edges(graph, relation)?);
    }
    Ok(edges)
}

fn relation_edges(graph: &impl GraphView, relation: RelationKind) -> Result<Vec<(String, String)>> {
    let (from_kind, to_kind) = relation.endpoints();
    let from_table = from_kind.table_name();
    let to_table = to_kind.table_name();
    let relation_table = relation.table_name();
    let rows = graph.query(
        &format!(
            "MATCH (from:`{from_table}`)-[:`{relation_table}`]->(to:`{to_table}`) RETURN from.id AS from_id, to.id AS to_id ORDER BY from.id, to.id;"
        ),
        &GraphParams::new(),
    )?;

    let mut edges = Vec::with_capacity(rows.len());
    for row in rows {
        edges.push((
            required_string(&row, "from_id")?,
            required_string(&row, "to_id")?,
        ));
    }
    edges.sort();
    Ok(edges)
}

fn relation_targets(
    edges: &BTreeMap<RelationKind, Vec<(String, String)>>,
    relations: &[RelationKind],
    from_id: &str,
) -> Vec<String> {
    let mut ids = BTreeSet::new();
    for relation in relations {
        if let Some(relation_edges) = edges.get(relation) {
            ids.extend(
                relation_edges
                    .iter()
                    .filter(|(from, _)| from == from_id)
                    .map(|(_, to)| to.clone()),
            );
        }
    }
    ids.into_iter().collect()
}

fn relation_sources(
    edges: &BTreeMap<RelationKind, Vec<(String, String)>>,
    relation: RelationKind,
    to_id: &str,
) -> Vec<String> {
    edges
        .get(&relation)
        .into_iter()
        .flat_map(|relation_edges| relation_edges.iter())
        .filter(|(_, to)| to == to_id)
        .map(|(from, _)| from.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn add_node_search_fields(
    fields: &mut Vec<SearchField>,
    rows: &BTreeMap<String, GraphRow>,
    kind: NodeKind,
    id: &str,
    field_specs: &[(&str, &str)],
) {
    for (field_name, property) in field_specs {
        let value = if *property == "id" {
            Some(id.to_owned())
        } else {
            rows.get(id).and_then(|row| optional_string(row, property))
        };
        if let Some(value) = value {
            fields.push(SearchField::node(field_name, &value, 3, kind, id));
        }
    }
}

fn evaluate_search_match(
    query: Option<&str>,
    terms: &[String],
    fields: &[SearchField],
) -> Option<SearchMatchInfo> {
    let Some(query) = query else {
        return Some(SearchMatchInfo {
            rank: 4,
            matched_fields: Vec::new(),
            snippets: Vec::new(),
            matched_nodes: Vec::new(),
        });
    };

    let mut matched_terms = BTreeSet::new();
    let mut matched_fields = BTreeSet::new();
    let mut snippets = Vec::new();
    let mut matched_nodes = BTreeSet::new();
    let mut rank = if exact_id_or_title_match(query, fields) {
        0
    } else {
        u8::MAX
    };

    for field in fields {
        let value_lower = field.value.to_ascii_lowercase();
        let mut field_matched = false;
        for term in terms {
            if value_lower.contains(term) {
                matched_terms.insert(term.clone());
                field_matched = true;
            }
        }

        if !field_matched {
            continue;
        }

        rank = rank.min(field.rank);
        matched_fields.insert(field.field.clone());
        if snippets.len() < MAX_SNIPPETS_PER_RESULT {
            snippets.push(SearchSnippet {
                field: field.field.clone(),
                value: snippet_value(&field.value),
            });
        }
        if let Some((kind, id)) = &field.node {
            matched_nodes.insert(SearchMatchedNode {
                id: id.clone(),
                kind: *kind,
                field: field.field.clone(),
            });
        }
    }

    if matched_terms.len() != terms.len() {
        return None;
    }

    Some(SearchMatchInfo {
        rank,
        matched_fields: matched_fields.into_iter().collect(),
        snippets,
        matched_nodes: matched_nodes.into_iter().collect(),
    })
}

fn exact_id_or_title_match(query: &str, fields: &[SearchField]) -> bool {
    let query = query.to_ascii_lowercase();
    fields.iter().any(|field| {
        matches!(field.field.as_str(), "decision.id" | "decision.title")
            && field.value.to_ascii_lowercase() == query
    })
}

fn snippet_value(value: &str) -> String {
    let mut snippet = value.chars().take(SNIPPET_MAX_CHARS).collect::<String>();
    if value.chars().count() > SNIPPET_MAX_CHARS {
        snippet.push_str("...");
    }
    snippet
}

fn decision_status_label(status: DecisionStatus) -> &'static str {
    match status {
        DecisionStatus::Proposed => "proposed",
        DecisionStatus::Accepted => "accepted",
        DecisionStatus::Rejected => "rejected",
        DecisionStatus::Contested => "contested",
        DecisionStatus::Superseded => "superseded",
    }
}

fn neighbor_pairs(
    graph: &impl GraphView,
    root_kind: NodeKind,
    root_id: &str,
    relation: RelationKind,
    other_kind: NodeKind,
    direction: Direction,
) -> Result<Vec<(String, Option<i64>)>> {
    let root_table = root_kind.table_name();
    let relation_table = relation.table_name();
    let other_table = other_kind.table_name();
    let cypher = match direction {
        Direction::Outgoing => format!(
            "MATCH (a:`{root_table}` {{id: $id}})-[r:`{relation_table}`]->(b:`{other_table}`) RETURN b.id AS id, r.event_origin AS event_origin ORDER BY b.id;"
        ),
        Direction::Incoming => format!(
            "MATCH (a:`{root_table}` {{id: $id}})<-[r:`{relation_table}`]-(b:`{other_table}`) RETURN b.id AS id, r.event_origin AS event_origin ORDER BY b.id;"
        ),
    };
    let rows = graph.query(
        &cypher,
        &GraphParams::from([("id".to_owned(), GraphValue::String(root_id.to_owned()))]),
    )?;

    let mut pairs = Vec::with_capacity(rows.len());
    for row in rows {
        let id = required_string(&row, "id")?;
        let event_origin = optional_int(&row, "event_origin");
        pairs.push((id, event_origin));
    }
    Ok(pairs)
}

pub fn derive_decision_status(graph: &impl GraphView, decision_id: &str) -> Result<DecisionStatus> {
    let superseded_count = relation_count(
        graph,
        RelationKind::Supersedes,
        Direction::Incoming,
        NodeKind::Decision,
        decision_id,
    )?;
    if superseded_count > 0 {
        return Ok(DecisionStatus::Superseded);
    }

    let accepted_count = relation_count(
        graph,
        RelationKind::AcceptedBy,
        Direction::Outgoing,
        NodeKind::Decision,
        decision_id,
    )?;
    let rejected_count = relation_count(
        graph,
        RelationKind::RejectedBy,
        Direction::Outgoing,
        NodeKind::Decision,
        decision_id,
    )?;

    match (accepted_count > 0, rejected_count > 0) {
        (true, true) => Ok(DecisionStatus::Contested),
        (true, false) => Ok(DecisionStatus::Accepted),
        (false, true) => Ok(DecisionStatus::Rejected),
        (false, false) => Ok(DecisionStatus::Proposed),
    }
}

pub fn derive_hypothesis_status(
    graph: &impl GraphView,
    hypothesis_id: &str,
) -> Result<HypothesisStatus> {
    let refuted_count = relation_count(
        graph,
        RelationKind::Refutes,
        Direction::Incoming,
        NodeKind::Hypothesis,
        hypothesis_id,
    )?;
    if refuted_count > 0 {
        return Ok(HypothesisStatus::Refuted);
    }

    let supported_count = relation_count(
        graph,
        RelationKind::Supports,
        Direction::Incoming,
        NodeKind::Hypothesis,
        hypothesis_id,
    )?;
    if supported_count > 0 {
        Ok(HypothesisStatus::Supported)
    } else {
        Ok(HypothesisStatus::Open)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Direction {
    Incoming,
    Outgoing,
}

fn relation_count(
    graph: &impl GraphView,
    relation: RelationKind,
    direction: Direction,
    node_kind: NodeKind,
    node_id: &str,
) -> Result<u64> {
    let relation_table = relation.table_name();
    let node_table = node_kind.table_name();
    let cypher = match direction {
        Direction::Incoming => format!(
            "MATCH (node:`{node_table}` {{id: $id}})<-[rel:`{relation_table}`]-() RETURN count(rel) AS count;"
        ),
        Direction::Outgoing => format!(
            "MATCH (node:`{node_table}` {{id: $id}})-[rel:`{relation_table}`]->() RETURN count(rel) AS count;"
        ),
    };
    let rows = graph.query(
        &cypher,
        &GraphParams::from([("id".to_owned(), GraphValue::String(node_id.to_owned()))]),
    )?;
    read_count(rows, relation_table)
}

fn neighbor_ids(
    graph: &impl GraphView,
    decision_id: &str,
    relation: RelationKind,
    to_kind: NodeKind,
    alias: &str,
) -> Result<Vec<String>> {
    let relation_table = relation.table_name();
    let to_table = to_kind.table_name();
    let cypher = format!(
        "MATCH (d:`Decision` {{id: $id}})-[:`{relation_table}`]->(n:`{to_table}`) RETURN n.id AS {alias} ORDER BY n.id;"
    );
    let rows = graph.query(
        &cypher,
        &GraphParams::from([("id".to_owned(), GraphValue::String(decision_id.to_owned()))]),
    )?;
    let mut ids = Vec::new();
    for row in rows {
        ids.push(required_string(&row, alias)?);
    }
    Ok(ids)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WalkDirection {
    Older,
    Newer,
}

fn supersession_neighbors(
    graph: &impl GraphView,
    decision_id: &str,
    direction: WalkDirection,
) -> Result<Vec<String>> {
    let cypher = match direction {
        WalkDirection::Older => {
            "MATCH (d:`Decision` {id: $id})-[:`SUPERSEDES`]->(other:`Decision`) RETURN other.id AS id ORDER BY other.id;"
        }
        WalkDirection::Newer => {
            "MATCH (other:`Decision`)-[:`SUPERSEDES`]->(d:`Decision` {id: $id}) RETURN other.id AS id ORDER BY other.id;"
        }
    };
    let rows = graph.query(
        cypher,
        &GraphParams::from([("id".to_owned(), GraphValue::String(decision_id.to_owned()))]),
    )?;

    let mut ids = Vec::new();
    for row in rows {
        ids.push(required_string(&row, "id")?);
    }
    Ok(ids)
}

fn choose_single_neighbor(current: &str, neighbors: Vec<String>) -> Result<Option<String>> {
    if neighbors.len() <= 1 {
        return Ok(neighbors.into_iter().next());
    }
    Err(query_error(format!(
        "supersession chain branched at {current}: {} candidates",
        neighbors.len()
    ))
    .into())
}

fn required_string(row: &GraphRow, key: &str) -> Result<String> {
    match row.get(key) {
        Some(GraphValue::String(value)) => Ok(value.clone()),
        _ => Err(query_error(format!("row missing string field: {key}")).into()),
    }
}

fn optional_string(row: &GraphRow, key: &str) -> Option<String> {
    match row.get(key) {
        Some(GraphValue::String(value)) => Some(value.clone()),
        _ => None,
    }
}

fn optional_int(row: &GraphRow, key: &str) -> Option<i64> {
    match row.get(key) {
        Some(GraphValue::Int(value)) => Some(*value),
        _ => None,
    }
}

fn required_datetime(row: &GraphRow, key: &str) -> Result<DateTime<Utc>> {
    optional_datetime(row, key)?
        .ok_or_else(|| query_error(format!("row missing timestamp field: {key}")).into())
}

fn optional_datetime(row: &GraphRow, key: &str) -> Result<Option<DateTime<Utc>>> {
    let Some(value) = optional_string(row, key) else {
        return Ok(None);
    };
    DateTime::parse_from_rfc3339(&value)
        .map(|value| Some(value.with_timezone(&Utc)))
        .map_err(|error| query_error(format!("invalid timestamp in {key}: {error}")).into())
}

fn optional_string_list(row: &GraphRow, key: &str) -> Vec<String> {
    match row.get(key) {
        Some(GraphValue::StringList(values)) => values.clone(),
        _ => Vec::new(),
    }
}

fn read_count(rows: Vec<GraphRow>, relation_table: &str) -> Result<u64> {
    let value = rows
        .first()
        .and_then(|row| row.get("count"))
        .ok_or_else(|| query_error(format!("{relation_table} count query returned no count")))?;

    match value {
        GraphValue::Int(value) if *value >= 0 => u64::try_from(*value).map_err(|error| {
            query_error(format!("{relation_table} count was invalid: {error}")).into()
        }),
        GraphValue::Int(value) => {
            Err(query_error(format!("{relation_table} count was negative: {value}")).into())
        }
        other => Err(query_error(format!(
            "{relation_table} count had unexpected value: {other:?}"
        ))
        .into()),
    }
}

fn query_error(error: impl std::fmt::Display) -> QueryError {
    QueryError::Execution(error.to_string())
}

fn query_timer_start() -> Instant {
    // ubs:ignore: Instant measures query latency only; it does not generate secrets.
    Instant::now()
}

#[cfg(test)]
mod tests;
