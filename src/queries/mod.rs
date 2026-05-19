use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use serde::Serialize;

use crate::error::QueryError;
use crate::projector::{GraphParams, GraphRow, GraphValue, GraphView, NodeKind, RelationKind};
use crate::Result;

const MAX_QUERY_RESULTS: usize = 1000;
const DEFAULT_SEARCH_LIMIT: usize = 25;
const MAX_SNIPPETS_PER_RESULT: usize = 5;
const SNIPPET_MAX_CHARS: usize = 160;

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
        NodeKind::Blocker => format!(
            "MATCH (node:`{table}`) RETURN node.id AS id, node.blocked_actor_id AS blocked_actor_id, node.decision_id AS decision_id, node.topic_keys AS topic_keys, node.blocked_ref AS blocked_ref, node.blocked_ref_type AS blocked_ref_type, node.reason AS reason, node.priority AS priority, node.last_progress_at AS last_progress_at, node.required_owner_id AS required_owner_id, node.source AS source, node.source_ref AS source_ref, node.event_origin AS event_origin ORDER BY node.id;"
        ),
        NodeKind::Evidence => format!(
            "MATCH (node:`{table}`) RETURN node.id AS id, node.content AS content, node.source AS source, node.source_ref AS source_ref, node.event_origin AS event_origin ORDER BY node.id;"
        ),
        NodeKind::Notification => format!(
            "MATCH (node:`{table}`) RETURN node.id AS id, node.blocker_id AS blocker_id, node.recipient_actor_id AS recipient_actor_id, node.channel AS channel, node.threshold_rule AS threshold_rule, node.source_event_ids AS source_event_ids, node.dedupe_key AS dedupe_key, node.sent_at AS sent_at, node.source AS source, node.source_ref AS source_ref, node.event_origin AS event_origin ORDER BY node.id;"
        ),
        NodeKind::Option => format!(
            "MATCH (node:`{table}`) RETURN node.id AS id, node.label AS label, node.description AS description, node.source AS source, node.source_ref AS source_ref, node.event_origin AS event_origin ORDER BY node.id;"
        ),
        NodeKind::Hypothesis => format!(
            "MATCH (node:`{table}`) RETURN node.id AS id, node.statement AS statement, node.source AS source, node.source_ref AS source_ref, node.event_origin AS event_origin ORDER BY node.id;"
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
mod tests {
    use std::cell::Cell;
    use std::collections::BTreeMap;
    use std::collections::BTreeSet;

    use crate::projector::GraphProperties;

    use super::*;

    #[derive(Debug, Default)]
    struct StatusGraph {
        edges: BTreeSet<(RelationKind, String, String)>,
        mutation_calls: Cell<usize>,
    }

    impl StatusGraph {
        fn with_edges(edges: &[(RelationKind, &str, &str)]) -> Self {
            Self {
                edges: edges
                    .iter()
                    .map(|(kind, from, to)| (*kind, (*from).to_owned(), (*to).to_owned()))
                    .collect(),
                mutation_calls: Cell::new(0),
            }
        }

        fn mutation_calls(&self) -> usize {
            self.mutation_calls.get()
        }
    }

    impl GraphView for StatusGraph {
        fn upsert_node(
            &self,
            _kind: NodeKind,
            _id: &str,
            _properties: &GraphProperties,
        ) -> Result<()> {
            self.mutation_calls.set(self.mutation_calls.get() + 1);
            Ok(())
        }

        fn upsert_edge(
            &self,
            _kind: RelationKind,
            _from_id: &str,
            _to_id: &str,
            _properties: &GraphProperties,
        ) -> Result<()> {
            self.mutation_calls.set(self.mutation_calls.get() + 1);
            Ok(())
        }

        fn query(&self, cypher: &str, params: &GraphParams) -> Result<Vec<GraphRow>> {
            let id = match params.get("id") {
                Some(GraphValue::String(id)) => id,
                _ => return Err(query_error("id param missing").into()),
            };
            let relation = relation_from_query(cypher)?;
            let incoming = cypher.contains("<-[rel:");
            let count = self
                .edges
                .iter()
                .filter(|(kind, from, to)| {
                    *kind == relation && if incoming { to == id } else { from == id }
                })
                .count();
            let count = i64::try_from(count)
                .map_err(|error| query_error(format!("count overflow: {error}")))?;

            Ok(vec![GraphRow::from([(
                "count".to_owned(),
                GraphValue::Int(count),
            )])])
        }

        fn wipe(&self) -> Result<()> {
            self.mutation_calls.set(self.mutation_calls.get() + 1);
            Ok(())
        }
    }

    #[test]
    fn derives_all_decision_status_cases() -> Result<()> {
        let cases = [
            (
                "proposed",
                StatusGraph::with_edges(&[(RelationKind::ProposedBy, "proposed", "actor:1")]),
                DecisionStatus::Proposed,
            ),
            (
                "accepted",
                StatusGraph::with_edges(&[(RelationKind::AcceptedBy, "accepted", "actor:1")]),
                DecisionStatus::Accepted,
            ),
            (
                "rejected",
                StatusGraph::with_edges(&[(RelationKind::RejectedBy, "rejected", "actor:1")]),
                DecisionStatus::Rejected,
            ),
            (
                "contested",
                StatusGraph::with_edges(&[
                    (RelationKind::AcceptedBy, "contested", "actor:1"),
                    (RelationKind::RejectedBy, "contested", "actor:2"),
                ]),
                DecisionStatus::Contested,
            ),
            (
                "superseded",
                StatusGraph::with_edges(&[
                    (RelationKind::AcceptedBy, "superseded", "actor:1"),
                    (RelationKind::RejectedBy, "superseded", "actor:2"),
                    (RelationKind::Supersedes, "newer", "superseded"),
                ]),
                DecisionStatus::Superseded,
            ),
        ];

        for (decision_id, graph, expected) in cases {
            assert_eq!(derive_decision_status(&graph, decision_id)?, expected);
            assert_eq!(graph.mutation_calls(), 0);
        }

        Ok(())
    }

    #[test]
    fn derives_all_hypothesis_status_cases() -> Result<()> {
        let cases = [
            ("open", StatusGraph::default(), HypothesisStatus::Open),
            (
                "supported",
                StatusGraph::with_edges(&[(RelationKind::Supports, "evidence:1", "supported")]),
                HypothesisStatus::Supported,
            ),
            (
                "refuted",
                StatusGraph::with_edges(&[
                    (RelationKind::Supports, "evidence:1", "refuted"),
                    (RelationKind::Refutes, "evidence:2", "refuted"),
                ]),
                HypothesisStatus::Refuted,
            ),
        ];

        for (hypothesis_id, graph, expected) in cases {
            assert_eq!(derive_hypothesis_status(&graph, hypothesis_id)?, expected);
            assert_eq!(graph.mutation_calls(), 0);
        }

        Ok(())
    }

    fn relation_from_query(cypher: &str) -> Result<RelationKind> {
        for relation in RelationKind::ALL {
            if cypher.contains(&format!("`{}`", relation.table_name())) {
                return Ok(relation);
            }
        }
        Err(query_error(format!("unknown relation in query: {cypher}")).into())
    }

    #[derive(Debug, Default)]
    struct FixtureGraph {
        decisions: BTreeMap<String, (String, String, Vec<String>)>,
        actors: BTreeSet<String>,
        evidence: BTreeMap<String, String>,
        options: BTreeMap<String, (String, String)>,
        hypotheses: BTreeMap<String, String>,
        edges: BTreeSet<(RelationKind, String, String)>,
    }

    impl FixtureGraph {
        fn sample() -> Self {
            let mut graph = Self::default();
            graph.decisions.insert(
                "d1".to_owned(),
                (
                    "Pick queue".to_owned(),
                    "Need reliability".to_owned(),
                    vec!["infra".to_owned(), "latency".to_owned()],
                ),
            );
            graph.decisions.insert(
                "d2".to_owned(),
                (
                    "Keep sync path".to_owned(),
                    "Prefer simplicity".to_owned(),
                    vec!["infra".to_owned()],
                ),
            );
            graph
                .hypotheses
                .insert("h1".to_owned(), "Queue improves p95".to_owned());
            graph
                .evidence
                .insert("e1".to_owned(), "Kuzu supports graph projection".to_owned());
            graph.options.insert(
                "o1".to_owned(),
                (
                    "Synchronous path".to_owned(),
                    "Keep request processing inline".to_owned(),
                ),
            );
            graph.options.insert(
                "o2".to_owned(),
                (
                    "Async queue".to_owned(),
                    "Use durable queued handoff".to_owned(),
                ),
            );
            graph.actors.extend(
                ["actor:1", "actor:2", "actor:3", "actor:4"]
                    .into_iter()
                    .map(str::to_owned),
            );

            graph.edges.insert((
                RelationKind::ProposedBy,
                "d1".to_owned(),
                "actor:1".to_owned(),
            ));
            graph.edges.insert((
                RelationKind::AcceptedBy,
                "d1".to_owned(),
                "actor:2".to_owned(),
            ));
            graph
                .edges
                .insert((RelationKind::HasOption, "d1".to_owned(), "o1".to_owned()));
            graph
                .edges
                .insert((RelationKind::HasOption, "d1".to_owned(), "o2".to_owned()));
            graph
                .edges
                .insert((RelationKind::Chose, "d1".to_owned(), "o2".to_owned()));
            graph
                .edges
                .insert((RelationKind::BasedOn, "d1".to_owned(), "e1".to_owned()));
            graph
                .edges
                .insert((RelationKind::Assumes, "d1".to_owned(), "h1".to_owned()));
            graph
                .edges
                .insert((RelationKind::Supports, "e1".to_owned(), "h1".to_owned()));

            graph.edges.insert((
                RelationKind::ProposedBy,
                "d2".to_owned(),
                "actor:3".to_owned(),
            ));
            graph.edges.insert((
                RelationKind::RejectedBy,
                "d2".to_owned(),
                "actor:4".to_owned(),
            ));
            graph
        }
    }

    impl GraphView for FixtureGraph {
        fn upsert_node(
            &self,
            _kind: NodeKind,
            _id: &str,
            _properties: &GraphProperties,
        ) -> Result<()> {
            Ok(())
        }

        fn upsert_edge(
            &self,
            _kind: RelationKind,
            _from_id: &str,
            _to_id: &str,
            _properties: &GraphProperties,
        ) -> Result<()> {
            Ok(())
        }

        fn query(&self, cypher: &str, params: &GraphParams) -> Result<Vec<GraphRow>> {
            if cypher.contains("RETURN node.id AS id") {
                if cypher.contains("`Decision`") {
                    return Ok(self
                        .decisions
                        .iter()
                        .map(|(id, (title, rationale, topics))| {
                            GraphRow::from([
                                ("id".to_owned(), GraphValue::String(id.clone())),
                                ("title".to_owned(), GraphValue::String(title.clone())),
                                (
                                    "rationale".to_owned(),
                                    GraphValue::String(rationale.clone()),
                                ),
                                (
                                    "topic_keys".to_owned(),
                                    GraphValue::StringList(topics.clone()),
                                ),
                                ("source".to_owned(), GraphValue::String("agent".to_owned())),
                            ])
                        })
                        .collect());
                }
                if cypher.contains("`Actor`") {
                    return Ok(self
                        .actors
                        .iter()
                        .map(|id| {
                            GraphRow::from([
                                ("id".to_owned(), GraphValue::String(id.clone())),
                                ("source".to_owned(), GraphValue::String("agent".to_owned())),
                                ("source_ref".to_owned(), GraphValue::String(id.clone())),
                            ])
                        })
                        .collect());
                }
                if cypher.contains("`Evidence`") {
                    return Ok(self
                        .evidence
                        .iter()
                        .map(|(id, content)| {
                            GraphRow::from([
                                ("id".to_owned(), GraphValue::String(id.clone())),
                                ("content".to_owned(), GraphValue::String(content.clone())),
                            ])
                        })
                        .collect());
                }
                if cypher.contains("`Option`") {
                    return Ok(self
                        .options
                        .iter()
                        .map(|(id, (label, description))| {
                            GraphRow::from([
                                ("id".to_owned(), GraphValue::String(id.clone())),
                                ("label".to_owned(), GraphValue::String(label.clone())),
                                (
                                    "description".to_owned(),
                                    GraphValue::String(description.clone()),
                                ),
                            ])
                        })
                        .collect());
                }
                if cypher.contains("`Hypothesis`") {
                    return Ok(self
                        .hypotheses
                        .iter()
                        .map(|(id, statement)| {
                            GraphRow::from([
                                ("id".to_owned(), GraphValue::String(id.clone())),
                                (
                                    "statement".to_owned(),
                                    GraphValue::String(statement.clone()),
                                ),
                            ])
                        })
                        .collect());
                }
            }

            if cypher.contains("RETURN from.id AS from_id, to.id AS to_id") {
                let relation = relation_from_query(cypher)?;
                return Ok(self
                    .edges
                    .iter()
                    .filter(|(kind, _, _)| *kind == relation)
                    .map(|(_, from, to)| {
                        GraphRow::from([
                            ("from_id".to_owned(), GraphValue::String(from.clone())),
                            ("to_id".to_owned(), GraphValue::String(to.clone())),
                        ])
                    })
                    .collect());
            }

            if cypher.contains("RETURN count(rel) AS count;") {
                let relation = relation_from_query(cypher)?;
                let incoming = cypher.contains("<-[rel:");
                let id = match params.get("id") {
                    Some(GraphValue::String(id)) => id,
                    _ => return Err(query_error("missing id param").into()),
                };
                let count = self
                    .edges
                    .iter()
                    .filter(|(kind, from, to)| {
                        *kind == relation && if incoming { to == id } else { from == id }
                    })
                    .count();
                return Ok(vec![GraphRow::from([(
                    "count".to_owned(),
                    GraphValue::Int(i64::try_from(count).unwrap_or(0)),
                )])]);
            }

            if cypher.contains("RETURN count(d) AS count;") {
                let topic = match params.get("topic") {
                    Some(GraphValue::String(topic)) => topic,
                    _ => return Err(query_error("missing topic param").into()),
                };
                let count = self
                    .decisions
                    .values()
                    .filter(|(_, _, topics)| topics.iter().any(|value| value == topic))
                    .count();
                return Ok(vec![GraphRow::from([(
                    "count".to_owned(),
                    GraphValue::Int(i64::try_from(count).unwrap_or(0)),
                )])]);
            }

            if cypher.contains("MATCH (d:`Decision` {id: $id}) RETURN d.id AS id") {
                let id = match params.get("id") {
                    Some(GraphValue::String(id)) => id,
                    _ => return Err(query_error("missing id param").into()),
                };
                if let Some((title, rationale, topics)) = self.decisions.get(id) {
                    return Ok(vec![GraphRow::from([
                        ("id".to_owned(), GraphValue::String(id.clone())),
                        ("title".to_owned(), GraphValue::String(title.clone())),
                        (
                            "rationale".to_owned(),
                            GraphValue::String(rationale.clone()),
                        ),
                        (
                            "topic_keys".to_owned(),
                            GraphValue::StringList(topics.clone()),
                        ),
                    ])]);
                }
                return Ok(Vec::new());
            }

            if cypher.contains("WHERE $topic IN d.topic_keys") {
                let topic = match params.get("topic") {
                    Some(GraphValue::String(topic)) => topic,
                    _ => return Err(query_error("missing topic param").into()),
                };
                let mut rows = self
                    .decisions
                    .iter()
                    .filter(|(_, (_, _, topics))| topics.iter().any(|value| value == topic))
                    .map(|(id, (title, rationale, topics))| {
                        GraphRow::from([
                            ("id".to_owned(), GraphValue::String(id.clone())),
                            ("title".to_owned(), GraphValue::String(title.clone())),
                            (
                                "rationale".to_owned(),
                                GraphValue::String(rationale.clone()),
                            ),
                            (
                                "topic_keys".to_owned(),
                                GraphValue::StringList(topics.clone()),
                            ),
                        ])
                    })
                    .collect::<Vec<_>>();
                rows.sort_by(|left, right| {
                    let l = left.get("id");
                    let r = right.get("id");
                    format!("{l:?}").cmp(&format!("{r:?}"))
                });
                return Ok(rows);
            }

            for (relation, alias) in [
                (RelationKind::HasOption, "option_id"),
                (RelationKind::Chose, "option_id"),
                (RelationKind::BasedOn, "evidence_id"),
                (RelationKind::Assumes, "hypothesis_id"),
            ] {
                if cypher.contains(&format!("[:`{}`]", relation.table_name())) {
                    let decision_id = match params.get("id") {
                        Some(GraphValue::String(id)) => id,
                        _ => return Err(query_error("missing id param").into()),
                    };
                    let mut ids = self
                        .edges
                        .iter()
                        .filter(|(kind, from, _)| *kind == relation && from == decision_id)
                        .map(|(_, _, to)| to.clone())
                        .collect::<Vec<_>>();
                    ids.sort();
                    return Ok(ids
                        .into_iter()
                        .map(|value| {
                            GraphRow::from([(alias.to_owned(), GraphValue::String(value))])
                        })
                        .collect());
                }
            }

            if cypher.contains("[r:`") && cypher.contains("AS event_origin") {
                let id = match params.get("id") {
                    Some(GraphValue::String(id)) => id,
                    _ => return Err(query_error("missing id param").into()),
                };
                let relation = relation_from_query(cypher)?;
                let incoming = cypher.contains("<-[r:`");
                let mut neighbors: Vec<String> = self
                    .edges
                    .iter()
                    .filter(|(kind, from, to)| {
                        *kind == relation && if incoming { to == id } else { from == id }
                    })
                    .map(
                        |(_, from, to)| {
                            if incoming {
                                from.clone()
                            } else {
                                to.clone()
                            }
                        },
                    )
                    .collect();
                neighbors.sort();
                return Ok(neighbors
                    .into_iter()
                    .map(|other_id| {
                        GraphRow::from([
                            ("id".to_owned(), GraphValue::String(other_id)),
                            ("event_origin".to_owned(), GraphValue::Null),
                        ])
                    })
                    .collect());
            }

            if cypher.contains("[:`SUPERSEDES`]->(other:`Decision`)") {
                let decision_id = match params.get("id") {
                    Some(GraphValue::String(id)) => id,
                    _ => return Err(query_error("missing id param").into()),
                };
                let mut ids = self
                    .edges
                    .iter()
                    .filter(|(kind, from, _)| {
                        *kind == RelationKind::Supersedes && from == decision_id
                    })
                    .map(|(_, _, to)| to.clone())
                    .collect::<Vec<_>>();
                ids.sort();
                return Ok(ids
                    .into_iter()
                    .map(|id| GraphRow::from([("id".to_owned(), GraphValue::String(id))]))
                    .collect());
            }

            if cypher.contains("(other:`Decision`)-[:`SUPERSEDES`]->(d:`Decision`") {
                let decision_id = match params.get("id") {
                    Some(GraphValue::String(id)) => id,
                    _ => return Err(query_error("missing id param").into()),
                };
                let mut ids = self
                    .edges
                    .iter()
                    .filter(|(kind, _, to)| *kind == RelationKind::Supersedes && to == decision_id)
                    .map(|(_, from, _)| from.clone())
                    .collect::<Vec<_>>();
                ids.sort();
                return Ok(ids
                    .into_iter()
                    .map(|id| GraphRow::from([("id".to_owned(), GraphValue::String(id))]))
                    .collect());
            }

            Err(query_error(format!("unsupported query in fixture: {cypher}")).into())
        }

        fn wipe(&self) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn get_decision_returns_neighbors_and_derived_status() -> Result<()> {
        let graph = FixtureGraph::sample();
        let response = get_decision(&graph, "d1")?;
        assert_eq!(response.result_count, 1);
        assert!(!response.truncated);
        let decision = response.data.expect("decision exists");
        assert_eq!(decision.id, "d1");
        assert_eq!(decision.status, DecisionStatus::Accepted);
        assert_eq!(decision.chosen_option_id.as_deref(), Some("o2"));
        assert_eq!(decision.option_ids, vec!["o1".to_owned(), "o2".to_owned()]);
        assert_eq!(decision.evidence_ids, vec!["e1".to_owned()]);
        assert_eq!(decision.hypotheses.len(), 1);
        assert_eq!(decision.hypotheses[0].id, "h1");
        assert_eq!(decision.hypotheses[0].status, HypothesisStatus::Supported);
        Ok(())
    }

    #[test]
    fn get_relevant_decisions_filters_by_status() -> Result<()> {
        let graph = FixtureGraph::sample();
        let response = get_relevant_decisions(&graph, "infra", Some(DecisionStatus::Rejected))?;
        assert_eq!(response.result_count, 1);
        assert_eq!(response.data.len(), 1);
        assert_eq!(response.data[0].id, "d2");
        assert_eq!(response.data[0].status, DecisionStatus::Rejected);
        Ok(())
    }

    #[test]
    fn search_decisions_matches_title_rationale_topic_status_and_actor() -> Result<()> {
        let graph = FixtureGraph::sample();
        let response = search_decisions(
            &graph,
            &SearchDecisionRequest {
                query: Some("queue".to_owned()),
                topic_keys: vec!["infra".to_owned()],
                statuses: vec![DecisionStatus::Accepted],
                actor_ids: vec!["actor:2".to_owned()],
                sources: vec!["agent".to_owned()],
                limit: 10,
                cursor: None,
            },
        )?;

        assert_eq!(response.result_count, 1);
        let result = &response.data.items[0];
        assert_eq!(result.decision.id, "d1");
        assert_eq!(result.decision.status, DecisionStatus::Accepted);
        assert_eq!(result.rank, 1);
        assert!(result.matched_fields.contains(&"decision.title".to_owned()));
        assert_eq!(result.graph_context.actor_ids, vec!["actor:1", "actor:2"]);

        let rationale_response = search_decisions(
            &graph,
            &SearchDecisionRequest {
                query: Some("simplicity".to_owned()),
                statuses: vec![DecisionStatus::Rejected],
                limit: 10,
                ..SearchDecisionRequest::default()
            },
        )?;
        assert_eq!(rationale_response.data.items[0].decision.id, "d2");
        assert!(rationale_response.data.items[0]
            .matched_fields
            .contains(&"decision.rationale".to_owned()));
        Ok(())
    }

    #[test]
    fn search_decisions_matches_graph_context_text() -> Result<()> {
        let graph = FixtureGraph::sample();
        for (query, field, kind) in [
            ("Kuzu", "evidence.content", NodeKind::Evidence),
            ("p95", "hypothesis.statement", NodeKind::Hypothesis),
            ("async", "option.label", NodeKind::Option),
        ] {
            let response = search_decisions(
                &graph,
                &SearchDecisionRequest {
                    query: Some(query.to_owned()),
                    limit: 10,
                    ..SearchDecisionRequest::default()
                },
            )?;

            assert_eq!(response.result_count, 1, "query {query}");
            let result = &response.data.items[0];
            assert_eq!(result.decision.id, "d1");
            assert_eq!(result.rank, 3);
            assert!(result.matched_fields.contains(&field.to_owned()));
            assert!(
                result
                    .graph_context
                    .matched_nodes
                    .iter()
                    .any(|node| node.kind == kind && node.field == field),
                "matched node missing for {field}"
            );
        }
        Ok(())
    }

    #[test]
    fn search_decisions_paginates_in_deterministic_order() -> Result<()> {
        let graph = FixtureGraph::sample();
        let first = search_decisions(
            &graph,
            &SearchDecisionRequest {
                topic_keys: vec!["infra".to_owned()],
                limit: 1,
                ..SearchDecisionRequest::default()
            },
        )?;

        assert_eq!(first.result_count, 1);
        assert!(first.truncated);
        assert_eq!(first.data.total_matches, 2);
        assert_eq!(first.data.items[0].decision.id, "d1");
        assert_eq!(first.data.next_cursor.as_deref(), Some("1"));

        let second = search_decisions(
            &graph,
            &SearchDecisionRequest {
                topic_keys: vec!["infra".to_owned()],
                limit: 1,
                cursor: first.data.next_cursor,
                ..SearchDecisionRequest::default()
            },
        )?;

        assert_eq!(second.result_count, 1);
        assert!(!second.truncated);
        assert_eq!(second.data.items[0].decision.id, "d2");
        assert_eq!(second.data.next_cursor, None);
        Ok(())
    }

    #[test]
    fn get_supersession_chain_walks_both_directions() -> Result<()> {
        let mut graph = FixtureGraph::sample();
        graph
            .edges
            .insert((RelationKind::Supersedes, "d2".to_owned(), "d1".to_owned()));
        graph
            .edges
            .insert((RelationKind::Supersedes, "d3".to_owned(), "d2".to_owned()));
        graph.decisions.insert(
            "d3".to_owned(),
            (
                "Newest".to_owned(),
                "latest".to_owned(),
                vec!["infra".to_owned()],
            ),
        );

        let chain = get_supersession_chain(&graph, "d2")?;
        assert_eq!(
            chain.data.decision_ids,
            vec!["d1".to_owned(), "d2".to_owned(), "d3".to_owned()]
        );
        assert_eq!(chain.data.input_index, 1);
        Ok(())
    }

    #[test]
    fn get_decision_neighborhood_returns_full_one_hop() -> Result<()> {
        let graph = FixtureGraph::sample();
        let response = get_decision_neighborhood(&graph, "d1", &NeighborhoodRequest::all())?;

        assert!(response.data.root.present);
        assert_eq!(response.data.root.id, "d1");
        assert_eq!(response.data.root.kind, NodeKind::Decision);

        let edge_relations: Vec<RelationKind> = response
            .data
            .edges
            .iter()
            .map(|edge| edge.relation)
            .collect();
        assert!(edge_relations.contains(&RelationKind::ProposedBy));
        assert!(edge_relations.contains(&RelationKind::AcceptedBy));
        assert!(edge_relations.contains(&RelationKind::HasOption));
        assert!(edge_relations.contains(&RelationKind::Chose));
        assert!(edge_relations.contains(&RelationKind::BasedOn));
        assert!(edge_relations.contains(&RelationKind::Assumes));
        // SUPPORTS arrives via 2-hop from the visible hypothesis h1 <- e1
        assert!(edge_relations.contains(&RelationKind::Supports));

        let node_ids: Vec<&str> = response.data.nodes.iter().map(|n| n.id.as_str()).collect();
        for expected in ["d1", "actor:1", "actor:2", "o1", "o2", "e1", "h1"] {
            assert!(node_ids.contains(&expected), "missing node {expected}");
        }

        let root_node = response
            .data
            .nodes
            .iter()
            .find(|n| n.id == "d1")
            .expect("root node present");
        assert_eq!(root_node.decision_status, Some(DecisionStatus::Accepted));

        let hypothesis_node = response
            .data
            .nodes
            .iter()
            .find(|n| n.id == "h1")
            .expect("hypothesis node present");
        assert_eq!(
            hypothesis_node.hypothesis_status,
            Some(HypothesisStatus::Supported)
        );

        let mut sorted = response.data.edges.clone();
        sorted.sort_by(|a, b| {
            (a.relation, &a.from, &a.to, a.event_origin).cmp(&(
                b.relation,
                &b.from,
                &b.to,
                b.event_origin,
            ))
        });
        assert_eq!(sorted, response.data.edges, "edges must be deterministic");

        Ok(())
    }

    #[test]
    fn get_decision_neighborhood_filters_by_relation() -> Result<()> {
        let graph = FixtureGraph::sample();
        let request = NeighborhoodRequest::with_relations([RelationKind::ProposedBy]);
        let response = get_decision_neighborhood(&graph, "d1", &request)?;

        for edge in &response.data.edges {
            assert_eq!(edge.relation, RelationKind::ProposedBy);
        }
        let node_ids: Vec<&str> = response.data.nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(node_ids.contains(&"actor:1"));
        assert!(!node_ids.contains(&"o1"), "options filtered out");
        assert!(!node_ids.contains(&"e1"), "evidence filtered out");
        Ok(())
    }

    #[test]
    fn get_decision_neighborhood_handles_missing_decision() -> Result<()> {
        let graph = FixtureGraph::sample();
        let response =
            get_decision_neighborhood(&graph, "no-such-decision", &NeighborhoodRequest::all())?;

        assert!(!response.data.root.present);
        assert!(response.data.nodes.is_empty());
        assert!(response.data.edges.is_empty());
        assert_eq!(response.result_count, 0);
        Ok(())
    }

    #[test]
    fn get_decision_neighborhood_reports_branched_supersession() -> Result<()> {
        let mut graph = FixtureGraph::sample();
        graph.decisions.insert(
            "branch_a".to_owned(),
            ("A".to_owned(), "rationale".to_owned(), Vec::new()),
        );
        graph.decisions.insert(
            "branch_b".to_owned(),
            ("B".to_owned(), "rationale".to_owned(), Vec::new()),
        );
        graph.edges.insert((
            RelationKind::Supersedes,
            "d1".to_owned(),
            "branch_a".to_owned(),
        ));
        graph.edges.insert((
            RelationKind::Supersedes,
            "d1".to_owned(),
            "branch_b".to_owned(),
        ));

        let response = get_decision_neighborhood(&graph, "d1", &NeighborhoodRequest::all())?;

        let supersedes_targets: Vec<&str> = response
            .data
            .edges
            .iter()
            .filter(|edge| edge.relation == RelationKind::Supersedes && edge.from == "d1")
            .map(|edge| edge.to.as_str())
            .collect();
        assert!(supersedes_targets.contains(&"branch_a"));
        assert!(supersedes_targets.contains(&"branch_b"));
        Ok(())
    }

    #[test]
    fn get_decision_neighborhood_includes_refuting_evidence_via_hypothesis() -> Result<()> {
        let mut graph = FixtureGraph::sample();
        graph
            .edges
            .insert((RelationKind::Refutes, "e2".to_owned(), "h1".to_owned()));

        let response = get_decision_neighborhood(&graph, "d1", &NeighborhoodRequest::all())?;

        let refutes_edges: Vec<&NeighborEdge> = response
            .data
            .edges
            .iter()
            .filter(|edge| edge.relation == RelationKind::Refutes)
            .collect();
        assert_eq!(refutes_edges.len(), 1);
        assert_eq!(refutes_edges[0].from, "e2");
        assert_eq!(refutes_edges[0].to, "h1");

        let node_ids: Vec<&str> = response.data.nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(node_ids.contains(&"e2"), "refuting evidence reached via h1");

        let hypothesis_node = response
            .data
            .nodes
            .iter()
            .find(|n| n.id == "h1")
            .expect("hypothesis node present");
        assert_eq!(
            hypothesis_node.hypothesis_status,
            Some(HypothesisStatus::Refuted)
        );
        Ok(())
    }

    #[test]
    fn get_decision_neighborhood_rejects_empty_id() {
        let graph = FixtureGraph::sample();
        let error = get_decision_neighborhood(&graph, "   ", &NeighborhoodRequest::all())
            .expect_err("empty id rejected");
        assert!(format!("{error}").contains("decision_id"));
    }

    #[test]
    fn get_supersession_chain_detects_cycle() {
        let mut graph = FixtureGraph::sample();
        graph
            .edges
            .insert((RelationKind::Supersedes, "d2".to_owned(), "d1".to_owned()));
        graph
            .edges
            .insert((RelationKind::Supersedes, "d1".to_owned(), "d2".to_owned()));

        let error = get_supersession_chain(&graph, "d1").expect_err("cycle should fail");
        assert!(format!("{error}").contains("cycle detected"));
    }
}
