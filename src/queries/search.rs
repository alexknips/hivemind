use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use serde::Serialize;

use crate::projector::{GraphRow, GraphView, NodeKind, RelationKind};
use crate::Result;

use super::decision::{DecisionView, HypothesisContext};
use super::shared::{
    node_rows, normalized_filter_values, normalized_limit, normalized_query, normalized_statuses,
    optional_string, optional_string_list, parse_cursor, query_terms, relation_edges_by_kind,
    relation_sources, relation_targets,
};
use super::status::{derive_decision_status, derive_hypothesis_status, DecisionStatus};
use super::QueryResponse;

const MAX_SNIPPETS_PER_RESULT: usize = 5;
const SNIPPET_MAX_CHARS: usize = 160;

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
            limit: super::shared::DEFAULT_SEARCH_LIMIT,
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
