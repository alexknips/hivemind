use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::Serialize;

use crate::events::{self, EventPayload};
use crate::ledger::{EventLedger, SqliteEventLedger};
use crate::projector::{GraphRow, GraphView, NodeKind, RelationKind};
use crate::Result;

use super::decision::{DecisionView, HypothesisContext};
use super::shared::{
    node_rows, normalized_filter_values, normalized_limit, normalized_query, normalized_statuses,
    optional_string, optional_string_list, parse_cursor, query_error, query_terms,
    relation_edges_by_kind, relation_sources, relation_targets,
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
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
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
            since: None,
            until: None,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<DateTime<Utc>>,
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
    if request.since.is_some() || request.until.is_some() {
        return Err(query_error("timestamp filters require FTS-backed decision search").into());
    }

    let mut scored = collect_graph_search_results(
        graph,
        query.as_deref(),
        &terms,
        &topic_keys,
        &statuses,
        &actor_ids,
        &sources,
    )?;

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
                since: request.since,
                until: request.until,
            },
            limit,
            cursor,
            next_cursor,
            total_matches,
            items,
        },
    })
}

pub fn search_decisions_fts(
    ledger: &SqliteEventLedger,
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
    let since = request.since;
    let until = request.until;
    if let (Some(since), Some(until)) = (since, until) {
        if since > until {
            return Err(query_error("--since must be earlier than or equal to --until").into());
        }
    }
    let limit = normalized_limit(request.limit);
    let cursor = normalized_query(request.cursor.as_deref());
    let offset = parse_cursor(cursor.as_deref())?;

    let documents = collect_graph_search_results(graph, None, &[], &[], &[], &[], &[])?;
    rebuild_decision_search_fts(ledger, &documents)?;
    let fts_matches = query_decision_search_fts(ledger, query.as_deref())?;
    let proposed_at = decision_proposed_at_by_id(ledger)?;

    let mut documents_by_id = documents
        .into_iter()
        .map(|document| (document.id.clone(), document))
        .collect::<BTreeMap<_, _>>();
    let mut scored = Vec::new();
    for fts_match in fts_matches {
        if !date_in_range(
            proposed_at.get(&fts_match.decision_id).copied(),
            since,
            until,
        ) {
            continue;
        }
        let Some(mut document) = documents_by_id.remove(&fts_match.decision_id) else {
            continue;
        };
        if !document_matches_filters(&document, &topic_keys, &statuses, &actor_ids, &sources) {
            continue;
        }

        let match_info = match query.as_deref() {
            Some(_) => evaluate_search_match(query.as_deref(), &terms, &document.fields)
                .unwrap_or_else(|| SearchMatchInfo {
                    rank: 3,
                    matched_fields: Vec::new(),
                    snippets: Vec::new(),
                    matched_nodes: Vec::new(),
                }),
            None => SearchMatchInfo {
                rank: 4,
                matched_fields: Vec::new(),
                snippets: Vec::new(),
                matched_nodes: Vec::new(),
            },
        };
        document.rank = match_info.rank;
        document.result.rank = match_info.rank;
        document.result.matched_fields = match_info.matched_fields;
        document.result.snippets = match_info.snippets;
        document.result.graph_context.matched_nodes = match_info.matched_nodes;
        scored.push(FtsScoredDecisionSearchResult {
            score: fts_match.score,
            id: document.id.clone(),
            result: document.result,
        });
    }

    scored.sort_by(|left, right| {
        left.score
            .total_cmp(&right.score)
            .then_with(|| left.id.cmp(&right.id))
    });

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
                since,
                until,
            },
            limit,
            cursor,
            next_cursor,
            total_matches,
            items,
        },
    })
}

struct FtsScoredDecisionSearchResult {
    score: f64,
    id: String,
    result: DecisionSearchResult,
}

struct FtsDecisionMatch {
    decision_id: String,
    score: f64,
}

fn rebuild_decision_search_fts(
    ledger: &SqliteEventLedger,
    documents: &[ScoredDecisionSearchResult],
) -> Result<()> {
    let mut connection = open_decision_search_connection(ledger)?;
    let transaction = connection
        .transaction()
        .map_err(|error| query_error(format!("begin decision search index rebuild: {error}")))?;
    transaction
        // ubs:ignore: static FTS schema SQL contains no request-controlled interpolation.
        .execute_batch(
            "DROP TABLE IF EXISTS decision_search_fts;
             CREATE VIRTUAL TABLE decision_search_fts USING fts5(
                 decision_id,
                 title,
                 rationale,
                 topic_keys,
                 status,
                 actor_text,
                 source,
                 option_text,
                 evidence_text,
                 hypothesis_text,
                 supersession_text,
                 tokenize = 'unicode61'
             );",
        )
        .map_err(|error| query_error(format!("initialize decision search index: {error}")))?;
    {
        let mut statement = transaction
            // ubs:ignore: static INSERT statement; values are bound via rusqlite params.
            .prepare(
                "INSERT INTO decision_search_fts (
                    decision_id,
                    title,
                    rationale,
                    topic_keys,
                    status,
                    actor_text,
                    source,
                    option_text,
                    evidence_text,
                    hypothesis_text,
                    supersession_text
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            )
            .map_err(|error| {
                query_error(format!("prepare decision search index insert: {error}"))
            })?;
        for document in documents {
            statement
                // ubs:ignore: document fields are bound parameters, not interpolated SQL.
                .execute(params![
                    field_text(&document.fields, &["decision.id"]),
                    field_text(&document.fields, &["decision.title"]),
                    field_text(&document.fields, &["decision.rationale"]),
                    field_text(&document.fields, &["decision.topic"]),
                    field_text(&document.fields, &["decision.status"]),
                    field_text(&document.fields, &["actor.id", "actor.source_ref"]),
                    field_text(&document.fields, &["decision.source"]),
                    field_text(
                        &document.fields,
                        &["option.id", "option.label", "option.description"],
                    ),
                    field_text(&document.fields, &["evidence.id", "evidence.content"]),
                    field_text(&document.fields, &["hypothesis.id", "hypothesis.statement"]),
                    field_text(&document.fields, &["supersedes.id", "superseded_by.id"]),
                ])
                .map_err(|error| {
                    query_error(format!(
                        "insert decision search index row for {}: {error}",
                        document.id
                    ))
                })?;
        }
    }
    transaction
        .commit()
        .map_err(|error| query_error(format!("commit decision search index rebuild: {error}")))?;
    Ok(())
}

fn query_decision_search_fts(
    ledger: &SqliteEventLedger,
    query: Option<&str>,
) -> Result<Vec<FtsDecisionMatch>> {
    let connection = open_decision_search_connection(ledger)?;
    let mut matches = Vec::new();
    if let Some(query) = query {
        let Some(fts_query) = fts5_query(query) else {
            return Ok(Vec::new());
        };
        let mut statement = connection
            // ubs:ignore: static SELECT statement; FTS query is bound as parameter ?1.
            .prepare(
                "SELECT decision_id,
                        bm25(decision_search_fts, 8.0, 5.0, 3.0, 2.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0) AS score
                   FROM decision_search_fts
                  WHERE decision_search_fts MATCH ?1
                  ORDER BY score ASC, decision_id ASC",
            )
            .map_err(|error| query_error(format!("prepare decision search query: {error}")))?;
        let rows = statement
            // ubs:ignore: FTS query value is bound through rusqlite params.
            .query_map(params![fts_query], |row| {
                Ok(FtsDecisionMatch {
                    decision_id: row.get(0)?,
                    score: row.get(1)?,
                })
            })
            .map_err(|error| query_error(format!("execute decision search query: {error}")))?;
        for row in rows {
            matches.push(row.map_err(|error| query_error(format!("read search row: {error}")))?);
        }
    } else {
        let mut statement = connection
            // ubs:ignore: static unfiltered SELECT contains no request-controlled interpolation.
            .prepare("SELECT decision_id FROM decision_search_fts ORDER BY decision_id ASC")
            .map_err(|error| query_error(format!("prepare unfiltered decision search: {error}")))?;
        let rows = statement
            // ubs:ignore: static unfiltered SELECT has no user-supplied SQL fragments.
            .query_map([], |row| {
                Ok(FtsDecisionMatch {
                    decision_id: row.get(0)?,
                    score: 0.0,
                })
            })
            .map_err(|error| query_error(format!("execute unfiltered decision search: {error}")))?;
        for row in rows {
            matches.push(row.map_err(|error| query_error(format!("read search row: {error}")))?);
        }
    }
    Ok(matches)
}

fn open_decision_search_connection(ledger: &SqliteEventLedger) -> Result<Connection> {
    // ubs:ignore: ledger.path() is a trusted local SQLite path, not request input.
    Ok(Connection::open(ledger.path())
        .map_err(|error| query_error(format!("open decision search index: {error}")))?)
}

fn decision_proposed_at_by_id(
    ledger: &impl EventLedger,
) -> Result<BTreeMap<String, DateTime<Utc>>> {
    let mut proposed_at = BTreeMap::new();
    ledger.replay_from(0, &mut |event| {
        let payload = events::validate(event)
            .map_err(|error| query_error(format!("invalid event during search replay: {error}")))?;
        if let EventPayload::DecisionProposed(payload) = payload {
            if let Some(ts) = event.ts {
                proposed_at.insert(payload.decision_id, ts);
            }
        }
        Ok(())
    })?;
    Ok(proposed_at)
}

fn date_in_range(
    proposed_at: Option<DateTime<Utc>>,
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
) -> bool {
    if since.is_none() && until.is_none() {
        return true;
    }
    let Some(proposed_at) = proposed_at else {
        return false;
    };
    since.is_none_or(|since| proposed_at >= since) && until.is_none_or(|until| proposed_at <= until)
}

fn document_matches_filters(
    document: &ScoredDecisionSearchResult,
    topic_keys: &[String],
    statuses: &[DecisionStatus],
    actor_ids: &[String],
    sources: &[String],
) -> bool {
    let decision = &document.result.decision;
    if !topic_keys.is_empty()
        && !topic_keys.iter().all(|topic| {
            decision
                .topic_keys
                .iter()
                .any(|candidate| candidate == topic)
        })
    {
        return false;
    }
    if !statuses.is_empty() && !statuses.contains(&decision.status) {
        return false;
    }
    if !actor_ids.is_empty()
        && !actor_ids.iter().any(|actor_id| {
            document
                .result
                .graph_context
                .actor_ids
                .iter()
                .any(|candidate| candidate == actor_id)
        })
    {
        return false;
    }
    if !sources.is_empty() {
        let source = field_text(&document.fields, &["decision.source"]);
        if !sources
            .iter()
            .any(|expected| source.eq_ignore_ascii_case(expected))
        {
            return false;
        }
    }
    true
}

fn field_text(fields: &[SearchField], field_names: &[&str]) -> String {
    fields
        .iter()
        .filter(|field| field_names.iter().any(|name| field.field == *name))
        .map(|field| field.value.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

fn fts5_query(query: &str) -> Option<String> {
    let terms = query
        .split(|character: char| !character.is_alphanumeric())
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>();
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" AND "))
    }
}

struct ScoredDecisionSearchResult {
    rank: u8,
    id: String,
    result: DecisionSearchResult,
    fields: Vec<SearchField>,
}

fn collect_graph_search_results(
    graph: &impl GraphView,
    query: Option<&str>,
    terms: &[String],
    topic_keys: &[String],
    statuses: &[DecisionStatus],
    actor_ids: &[String],
    sources: &[String],
) -> Result<Vec<ScoredDecisionSearchResult>> {
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

        let Some(match_info) = evaluate_search_match(query, terms, &fields) else {
            continue;
        };

        scored.push(ScoredDecisionSearchResult {
            rank: match_info.rank,
            id: id.clone(),
            fields,
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

    Ok(scored)
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
