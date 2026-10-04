//! Full-text and filter search over decisions via SQLite FTS and graph predicate evaluation.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::Instant;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::Serialize;

use crate::events::{self, EventPayload, ReadEvent};
use crate::ledger::{AnyLedger, EventLedger, SqliteEventLedger, TenantScopedLedger};
use crate::projector::{GraphRow, GraphView, NodeKind, RelationKind};
use crate::Result;

use super::decision::{DecisionView, HypothesisContext};
use super::grounding::{hypothesis_facts_from_row, GroundingState};
use super::project_label::ProjectLabels;
use super::project_scope::{project_scope, MatchScope, ProjectScope, ScopeNote, ScopeRelation};
use super::same_as::{fold_linked, RecordedCopy};
use super::shared::{
    node_rows, normalized_filter_values, normalized_limit, normalized_query, normalized_statuses,
    optional_int, optional_string, optional_string_list, parse_cursor, query_error, query_terms,
    relation_edges_by_kind, relation_sources, relation_targets,
};
use super::status::{derive_decision_status, derive_hypothesis_status, DecisionStatus};
use super::terms::{
    content_query, is_negated_text, resolver_question, stem, word_stems, RelatedWord, WordMatch,
};
use super::{QueryContext, QueryResponse};

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
    /// Ask from this project: a registered handle or a personal address. Results are limited to
    /// decisions filed under the project, the project it is part of, and the projects it depends
    /// on, in that order (own first, then the parent's, then a dependency's; see
    /// `project_scope`), each in the usual rank order. `None` searches the whole tenant, as
    /// before. An unregistered handle is refused, never an empty answer. Only the ledger-backed
    /// searches resolve a project; the graph-only `search_decisions` refuses one.
    pub project: Option<String>,
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
            project: None,
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
    /// Where a project-scoped answer looked and where it stopped. Absent when the request named
    /// no project.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<ScopeNote>,
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
    /// Terms of the question this decision does not contain. Empty for a full match; non-empty
    /// only for the close matches `recall` returns after the full ones, so a partial answer is
    /// never presented as a complete one.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub missing_terms: Vec<String>,
    pub snippets: Vec<SearchSnippet>,
    pub graph_context: SearchGraphContext,
    /// How this decision reached a project-scoped answer (own project, inherited from the
    /// parent, or from a dependency). Absent when the request named no project.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<MatchScope>,
    /// Other records of this same decision, linked `SAME_AS`: set only by `recall`, which shows a
    /// decision recorded more than once as one item (hivemind-83cj). `search` lists records.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub also_recorded_as: Vec<RecordedCopy>,
}

impl DecisionSearchResult {
    /// Takes the match of another record of the same decision onto this one: the best rank, the
    /// fields either matched, and only the words neither record lacks.
    fn absorb_record(&mut self, other: Self) {
        self.rank = self.rank.min(other.rank);
        for field in other.matched_fields {
            if !self.matched_fields.contains(&field) {
                self.matched_fields.push(field);
            }
        }
        self.missing_terms
            .retain(|term| other.missing_terms.contains(term));
    }
}

/// Shows each decision recorded more than once, linked `SAME_AS`, as one result: the record made
/// first among those that matched, carrying the best match of any of them and naming the other
/// records in `also_recorded_as` (hivemind-83cj). Results with no link pass through unchanged,
/// and nothing is folded on closeness.
pub(crate) fn fold_linked_results(
    graph: &impl GraphView,
    results: Vec<DecisionSearchResult>,
) -> Result<Vec<DecisionSearchResult>> {
    Ok(fold_linked(
        graph,
        results,
        |result| result.decision.id.as_str(),
        DecisionSearchResult::absorb_record,
    )?
    .into_iter()
    .map(|folded| DecisionSearchResult {
        also_recorded_as: folded.also_recorded_as,
        ..folded.item
    })
    .collect())
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SearchSnippet {
    pub field: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SearchGraphContext {
    pub actor_ids: Vec<String>,
    /// The human whose delegated scope an agent's self-acceptance fell within
    /// (hivemind-zdsh.6), read off the Decision node's `delegated_by` property.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delegated_by: Option<String>,
    pub supersedes_decision_ids: Vec<String>,
    pub superseded_by_decision_ids: Vec<String>,
    pub option_ids: Vec<String>,
    pub evidence_ids: Vec<String>,
    pub hypotheses: Vec<HypothesisContext>,
    /// Whether anything was declared about what this decision rests on.
    pub grounding_state: GroundingState,
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
    // The project structure lives in the ledger, which this graph-only search never sees;
    // refuse rather than answer as though the whole tenant were the project.
    if request.project.is_some() {
        return Err(query_error("project scoping requires ledger-backed decision search").into());
    }

    let mut scored = collect_graph_search_results(
        graph,
        query.as_deref(),
        &SearchTerms::literal(&terms),
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
            scope: None,
        },
    })
}

pub fn search_decisions_fts(
    ledger: &SqliteEventLedger,
    graph: &impl GraphView,
    request: &SearchDecisionRequest,
) -> Result<QueryResponse<DecisionSearchResults>> {
    search_decisions_fts_with_context(&QueryContext::local(), ledger, graph, request)
}

/// Search decisions using in-memory graph matching with optional ledger-backed timestamp filtering.
///
/// This is the backend-agnostic search path: it works with any `EventLedger` (SQLite or Postgres)
/// and uses in-memory term matching rather than SQLite FTS. Ranking is by match rank + decision id —
/// the same ordinal scheme `search_decisions_fts_with_context` uses, so the two backends return
/// identical order for identical fixtures (see docs/SEARCH_DESIGN.md's Ordering Guarantees).
pub fn search_decisions_with_ledger(
    context: &QueryContext,
    ledger: &impl EventLedger,
    graph: &impl GraphView,
    request: &SearchDecisionRequest,
) -> Result<QueryResponse<DecisionSearchResults>> {
    search_with_ledger(context, ledger, graph, request, Matching::Literal)
}

/// Search for what a question asks about (the `recall` path): the request's text is already
/// stripped of question words, and a term matches a field word with the same stem as well as a
/// substring. A decision matching every term comes first; then, fewest missing terms first, the
/// ones matching at least half of them, each carrying `missing_terms` so a partial answer is
/// never presented as a complete one. Filters, project scope, ordering ties and pagination are
/// those of `search_decisions_with_ledger`.
///
/// `negated` says the question was asked with a negation ("why doesn't ..."), which the request's
/// text no longer carries. It never narrows the answer and is never a missing term: among
/// decisions that match equally, one whose own title is negated comes first.
///
/// One in-memory path for every backend: SQLite's FTS5 only matches whole tokens (every term,
/// exactly as written), which is the strictness this exists to relax.
pub fn search_decisions_fluent(
    context: &QueryContext,
    ledger: &impl EventLedger,
    graph: &impl GraphView,
    request: &SearchDecisionRequest,
    negated: bool,
) -> Result<QueryResponse<DecisionSearchResults>> {
    search_with_ledger(
        context,
        ledger,
        graph,
        request,
        Matching::Fluent { negated },
    )
}

/// How a search request's free text is matched against a decision.
#[derive(Clone, Copy)]
enum Matching {
    /// Every term as a substring (`search`).
    Literal,
    /// Stemmed terms, close matches after the full ones (`recall`); `negated` is the polarity of
    /// the question.
    Fluent { negated: bool },
}

fn search_with_ledger(
    context: &QueryContext,
    ledger: &impl EventLedger,
    graph: &impl GraphView,
    request: &SearchDecisionRequest,
    matching: Matching,
) -> Result<QueryResponse<DecisionSearchResults>> {
    let started = Instant::now();
    let query = normalized_query(request.query.as_deref());
    let terms = query_terms(query.as_deref());
    let search_terms = match matching {
        Matching::Literal => SearchTerms::literal(&terms),
        Matching::Fluent { negated } => SearchTerms::fluent(&terms, negated),
    };
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
    let scope = resolve_scope(context, ledger, request.project.as_deref())?;

    let proposed_at = if since.is_some() || until.is_some() {
        Some(decision_proposed_at_by_id(context, ledger)?)
    } else {
        None
    };

    let mut scored = collect_graph_search_results(
        graph,
        query.as_deref(),
        &search_terms,
        &topic_keys,
        &statuses,
        &actor_ids,
        &sources,
    )?;

    if let Some(ref proposed_at_map) = proposed_at {
        scored.retain(|doc| date_in_range(proposed_at_map.get(&doc.id).copied(), since, until));
    }

    let scope_note = match &scope {
        Some(scope) => Some(narrow_to_scope(&mut scored, scope, graph)?),
        None => None,
    };
    sort_scored(&mut scored);

    let total_matches = scored.len();
    let items: Vec<DecisionSearchResult> = scored
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|s| s.result)
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
            scope: scope_note,
        },
    })
}

/// How many words make a query a phrase (`search_decisions_any`).
const PHRASE_WORDS: usize = 3;

/// Backend-dispatching search: SQLite goes through FTS
/// (`search_decisions_fts_with_context`); Postgres goes through the
/// backend-agnostic in-memory path (`search_decisions_with_ledger`). Both
/// return identical order for identical fixtures (see docs/SEARCH_DESIGN.md's
/// Ordering Guarantees), so callers that only hold an `AnyLedger` — the CLI,
/// stdio MCP, and one-shot HTTP paths — never need to know which backend they
/// are on.
///
/// A phrase of at least `PHRASE_WORDS` words that no decision matches in full is asked again the
/// way `recall` asks (`search_decisions_fluent`), so what an agent wrote in its own words still
/// finds the decision that holds some of them. That answer says what each decision lacks
/// (`DecisionSearchResult::missing_terms`) and echoes the query as it was given; a query some
/// decision matches in full, or of fewer words (a short query stays exact), is answered exactly
/// as before.
pub fn search_decisions_any(
    context: &QueryContext,
    ledger: &AnyLedger,
    graph: &impl GraphView,
    request: &SearchDecisionRequest,
) -> Result<QueryResponse<DecisionSearchResults>> {
    let strict = match ledger {
        AnyLedger::Sqlite(inner) => {
            search_decisions_fts_with_context(context, inner, graph, request)?
        }
        #[cfg(feature = "shared-backend-postgres")]
        AnyLedger::Postgres(inner) => search_decisions_with_ledger(context, inner, graph, request)?,
    };
    if strict.data.total_matches > 0 {
        return Ok(strict);
    }
    let Some(content) = request.query.as_deref().map(content_query) else {
        return Ok(strict);
    };
    if query_terms(content.query.as_deref()).len() < PHRASE_WORDS {
        return Ok(strict);
    }
    let fluent_request = SearchDecisionRequest {
        query: content.query,
        ..request.clone()
    };
    let mut fluent =
        search_decisions_fluent(context, ledger, graph, &fluent_request, content.negated)?;
    fluent.data.query = strict.data.query;
    Ok(fluent)
}

pub fn search_decisions_fts_with_context(
    context: &QueryContext,
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
    let scope = resolve_scope(context, ledger, request.project.as_deref())?;

    let documents =
        collect_graph_search_results(graph, None, &SearchTerms::literal(&[]), &[], &[], &[], &[])?;
    rebuild_decision_search_fts(ledger, &documents)?;
    let fts_decision_ids = query_decision_search_fts(ledger, query.as_deref())?;
    let proposed_at = decision_proposed_at_by_id(context, ledger)?;

    let mut documents_by_id = documents
        .into_iter()
        .map(|document| (document.id.clone(), document))
        .collect::<BTreeMap<_, _>>();
    let mut scored = Vec::new();
    for decision_id in fts_decision_ids {
        if !date_in_range(proposed_at.get(&decision_id).copied(), since, until) {
            continue;
        }
        let Some(mut document) = documents_by_id.remove(&decision_id) else {
            continue;
        };
        if !document_matches_filters(&document, &topic_keys, &statuses, &actor_ids, &sources) {
            continue;
        }

        let match_info = match query.as_deref() {
            Some(_) => evaluate_search_match(
                query.as_deref(),
                &SearchTerms::literal(&terms),
                &document.fields,
            )
            .unwrap_or_else(|| SearchMatchInfo {
                rank: 3,
                ..SearchMatchInfo::default()
            }),
            None => SearchMatchInfo {
                rank: 4,
                ..SearchMatchInfo::default()
            },
        };
        document.rank = match_info.rank;
        document.result.rank = match_info.rank;
        document.result.matched_fields = match_info.matched_fields;
        document.result.snippets = match_info.snippets;
        document.result.graph_context.matched_nodes = match_info.matched_nodes;
        scored.push(document);
    }

    // Same ordinal (rank, id) ordering as search_decisions_with_ledger (the Postgres/generic
    // graph path): FTS5's MATCH clause above only selects the candidate set, never the order.
    // BM25 values are a SQLite-internal retrieval detail (docs/SEARCH_DESIGN.md's
    // Storage-Bound Behaviors) — keeping them out of the sort key is what gives both backends
    // identical order for identical fixtures. A project scope is the same filter on the
    // decision's project after FTS, so both backends narrow and group identically.
    let scope_note = match &scope {
        Some(scope) => Some(narrow_to_scope(&mut scored, scope, graph)?),
        None => None,
    };
    sort_scored(&mut scored);

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
            scope: scope_note,
        },
    })
}

/// The project structure a scoped request is answered from, read once from the ledger; `None`
/// for an unscoped request. An unregistered handle is refused here, before any search work.
fn resolve_scope(
    context: &QueryContext,
    ledger: &impl EventLedger,
    project: Option<&str>,
) -> Result<Option<ProjectScope>> {
    let Some(project) = project else {
        return Ok(None);
    };
    let scoped_ledger = TenantScopedLedger::new(ledger, context.tenant_id.clone());
    project_scope(&scoped_ledger, project).map(Some)
}

/// Keep the documents filed under a project in `scope` and record how each one got there. A
/// decision with no project, or in a project the scope did not follow, is left out; the returned
/// note says which projects were looked in and where the walk stopped, so a short answer never
/// reads as a complete one. This is a filter on the decision's own `project`: one lookup per
/// document, no walk.
fn narrow_to_scope(
    scored: &mut Vec<ScoredDecisionSearchResult>,
    scope: &ProjectScope,
    graph: &impl GraphView,
) -> Result<ScopeNote> {
    let labels = ProjectLabels::from_graph(graph)?;
    scored.retain_mut(|document| {
        let decision = &document.result.decision;
        let Some(relation) = scope.relation_of(decision.project.as_deref()) else {
            return false;
        };
        let match_scope = scope.match_scope(relation, &decision.project_label, &labels);
        document.relation = Some(relation);
        document.result.scope = Some(match_scope);
        true
    });
    Ok(scope.note(&labels))
}

/// Own project first, then the parent's, then a dependency's; within each, the decisions that
/// lack the fewest of the question's terms, then (among close matches) the ones whose title or
/// topic keys carry more of the terms they did match, then (for a decision below the bar, see
/// `admit_below_bar`) the one that names a word it holds in both its title and its topic keys, then
/// the ones that match fewer of them only
/// through a stand-in word (a decision that has the word asked for comes before one that has a
/// synonym), then (rank, id) order. A negated question
/// does not change that order: among decisions tied on all of it, the ones whose title is negated
/// too come before the ones whose title is not. An unscoped document has no relation and a literal
/// match lacks nothing and is never negated, so an unscoped `search` keeps the plain (rank, id)
/// order.
fn sort_scored(scored: &mut [ScoredDecisionSearchResult]) {
    scored.sort_by(|left, right| {
        (
            left.relation,
            left.result.missing_terms.len(),
            Reverse(left.headline_terms),
            Reverse(left.headline_hits),
            left.stand_ins,
            left.rank,
            left.polarity_mismatch,
            &left.id,
        )
            .cmp(&(
                right.relation,
                right.result.missing_terms.len(),
                Reverse(right.headline_terms),
                Reverse(right.headline_hits),
                right.stand_ins,
                right.rank,
                right.polarity_mismatch,
                &right.id,
            ))
    });
}

// ubs:ignore: This helper only executes static FTS SQL and uses rusqlite params! for document values.
fn rebuild_decision_search_fts(
    // ubs:ignore: FTS rebuild uses static SQL plus rusqlite parameter binding; ledger is not interpolated SQL.
    ledger: &SqliteEventLedger,
    // ubs:ignore: FTS rebuild uses static SQL plus rusqlite parameter binding; documents are bound as parameters.
    documents: &[ScoredDecisionSearchResult],
    // ubs:ignore: FTS rebuild uses static SQL plus rusqlite parameter binding.
) -> Result<()> {
    let mut connection = open_decision_search_connection(ledger)?;
    let transaction = connection
        .transaction()
        .map_err(|error| query_error(format!("begin decision search index rebuild: {error}")))?;
    // ubs:ignore: FTS schema SQL is static; no user input is interpolated into this statement.
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
                 question_text,
                 tokenize = 'unicode61'
             );",
        )
        .map_err(|error| query_error(format!("initialize decision search index: {error}")))?;
    {
        // ubs:ignore: FTS insert SQL is static and dynamic values are bound with rusqlite params!.
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
                    supersession_text,
                    question_text
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
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
                    field_text(&document.fields, &["decision.question"]),
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

/// Returns the decision ids FTS5 considers a match for `query` (or every indexed decision id
/// when `query` is `None`), in no particular order — the caller re-ranks by the shared
/// (rank, id) ordinal scheme, so FTS5 here only selects the candidate set.
fn query_decision_search_fts(
    // ubs:ignore: FTS query uses static SQL with MATCH bound via rusqlite params.
    ledger: &SqliteEventLedger,
    // ubs:ignore: Caller text is converted to an FTS expression and bound as ?1.
    query: Option<&str>,
    // ubs:ignore: SQL text in this function is static and parameterized.
) -> Result<Vec<String>> {
    let connection = open_decision_search_connection(ledger)?;
    let mut decision_ids = Vec::new();
    // ubs:ignore: Query text is converted to an FTS expression and bound as ?1 below.
    if let Some(query) = query {
        let Some(fts_query) = fts5_query(query) else {
            return Ok(Vec::new());
        };
        // ubs:ignore: SQL statement is static; the FTS expression is passed via params![fts_query].
        let mut statement = connection
            // ubs:ignore: static SELECT statement; FTS query is bound as parameter ?1.
            .prepare(
                "SELECT decision_id
                   FROM decision_search_fts
                  WHERE decision_search_fts MATCH ?1
                  ORDER BY decision_id ASC",
            )
            .map_err(|error| query_error(format!("prepare decision search query: {error}")))?;
        let rows = statement
            // ubs:ignore: FTS query value is bound through rusqlite params.
            .query_map(params![fts_query], |row| row.get::<_, String>(0))
            .map_err(|error| query_error(format!("execute decision search query: {error}")))?;
        for row in rows {
            decision_ids
                .push(row.map_err(|error| query_error(format!("read search row: {error}")))?);
        }
    } else {
        // ubs:ignore: SQL statement is static and has no caller-controlled interpolation.
        let mut statement = connection
            // ubs:ignore: static unfiltered SELECT contains no request-controlled interpolation.
            .prepare("SELECT decision_id FROM decision_search_fts ORDER BY decision_id ASC")
            .map_err(|error| query_error(format!("prepare unfiltered decision search: {error}")))?;
        let rows = statement
            // ubs:ignore: static unfiltered SELECT has no user-supplied SQL fragments.
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| query_error(format!("execute unfiltered decision search: {error}")))?;
        for row in rows {
            decision_ids
                .push(row.map_err(|error| query_error(format!("read search row: {error}")))?);
        }
    }
    Ok(decision_ids)
}

fn open_decision_search_connection(ledger: &SqliteEventLedger) -> Result<Connection> {
    // ubs:ignore: ledger.path() is a trusted local SQLite path, not request input.
    Ok(Connection::open(ledger.path())
        .map_err(|error| query_error(format!("open decision search index: {error}")))?)
}

fn decision_proposed_at_by_id(
    context: &QueryContext,
    ledger: &impl EventLedger,
) -> Result<BTreeMap<String, DateTime<Utc>>> {
    let mut proposed_at = BTreeMap::new();
    ledger.replay_from_for_tenant(&context.tenant_id, 0, &mut |event| {
        let ReadEvent::Payload(payload) = events::validate_for_read(event)
            .map_err(|error| query_error(format!("invalid event during search replay: {error}")))?
        else {
            return Ok(());
        };
        if let EventPayload::DecisionProposed(payload) = *payload {
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
    /// How the decision's project relates to the asked project; `None` for an unscoped request.
    relation: Option<ScopeRelation>,
    rank: u8,
    id: String,
    event_origin: i64,
    /// `SearchMatchInfo::polarity_mismatch`.
    polarity_mismatch: bool,
    /// `SearchMatchInfo::headline_terms`: for a close match, how many of the terms its title or
    /// topic keys contain.
    headline_terms: usize,
    /// `SearchMatchInfo::headline_hits` for a decision below the bar, else 0.
    headline_hits: usize,
    /// `SearchMatchInfo::stand_in_terms`.
    stand_ins: usize,
    result: DecisionSearchResult,
    fields: Vec<SearchField>,
}

fn collect_graph_search_results(
    graph: &impl GraphView,
    query: Option<&str>,
    terms: &SearchTerms<'_>,
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
    let question_rows = node_rows(graph, NodeKind::Question)?;
    let edges = relation_edges_by_kind(graph)?;
    let labels = ProjectLabels::from_graph(graph)?;

    let mut scored = Vec::new();
    // What `admit_below_bar` needs of each decision in `scored`, in the same order.
    let mut holdings: Vec<Holding> = Vec::new();
    // The decisions that passed the filters: the ledger the terms are weighed against.
    let mut searched = 0_usize;
    for (id, row) in decision_rows {
        let project = optional_string(&row, "project");
        let project_label = labels.label_of(project.as_deref());
        let title = optional_string(&row, "title").unwrap_or_default();
        let rationale = optional_string(&row, "rationale").unwrap_or_default();
        let quote = optional_string(&row, "quote");
        let question = optional_string(&row, "question");
        let delegated_by = optional_string(&row, "delegated_by");
        let event_origin = optional_int(&row, "event_origin").unwrap_or(0);
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
        let mut hypothesis_ids = relation_targets(&edges, &[RelationKind::PremisedOnDirect], &id);
        if let Some(opt_id) = &chosen_option_id {
            hypothesis_ids.extend(relation_targets(
                &edges,
                &[RelationKind::PremisedOn],
                opt_id,
            ));
            hypothesis_ids.sort();
            hypothesis_ids.dedup();
        }
        let supersedes_decision_ids = relation_targets(&edges, &[RelationKind::Supersedes], &id);
        let superseded_by_decision_ids = relation_sources(&edges, RelationKind::Supersedes, &id);
        let premise_decision_ids = relation_targets(&edges, &[RelationKind::FollowsFrom], &id);
        let question_id = relation_targets(&edges, &[RelationKind::Answers], &id)
            .into_iter()
            .next();

        let mut hypotheses = Vec::with_capacity(hypothesis_ids.len());
        for hypothesis_id in &hypothesis_ids {
            let facts = match hypothesis_rows.get(hypothesis_id) {
                Some(row) => hypothesis_facts_from_row(row)?,
                None => hypothesis_facts_from_row(&GraphRow::new())?,
            };
            hypotheses.push(HypothesisContext {
                id: hypothesis_id.clone(),
                status: derive_hypothesis_status(graph, hypothesis_id)?,
                kind: facts.kind,
                check_by: facts.check_by,
                would_change_if: facts.would_change_if,
            });
        }

        // A decision linked to a question after the fact has no question text of its own.
        let question = question.or_else(|| {
            question_id
                .as_ref()
                .and_then(|question_id| question_rows.get(question_id))
                .and_then(|row| optional_string(row, "text"))
        });

        let mut fields = Vec::new();
        fields.push(SearchField::decision("decision.id", &id, 0));
        fields.push(SearchField::decision("decision.title", &title, 1));
        fields.push(SearchField::decision("decision.rationale", &rationale, 2));
        // The question a decision answers is often the only text a person would ask with: a
        // decision the ask hooks write is titled "<header>: <choice>" with a fixed rationale.
        if let Some(question) = &question {
            fields.push(SearchField::decision("decision.question", question, 2));
        }
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

        searched += 1;
        let Some(match_info) = evaluate_search_match(query, terms, &fields) else {
            continue;
        };
        holdings.push(Holding {
            below_bar: match_info.below_bar,
            held: match_info.held_words,
            headline: match_info.headline_words,
        });

        let decision = DecisionView {
            id: id.clone(),
            title,
            rationale,
            topic_keys: decision_topic_keys,
            status,
            project,
            project_label,
            chosen_option_id,
            option_ids: option_ids.clone(),
            evidence_ids: evidence_ids.clone(),
            hypotheses: hypotheses.clone(),
            premise_decision_ids,
            quote: quote.clone(), // ubs:ignore: clone necessary — building owned DecisionView
            question,
            question_id,
        };
        let grounding_state = decision.grounding_state();

        scored.push(ScoredDecisionSearchResult {
            relation: None,
            rank: match_info.rank,
            id,
            event_origin,
            polarity_mismatch: match_info.polarity_mismatch,
            headline_terms: match_info.headline_terms,
            headline_hits: if match_info.below_bar {
                match_info.headline_hits
            } else {
                0
            },
            stand_ins: match_info.stand_in_terms,
            fields,
            result: DecisionSearchResult {
                decision,
                rank: match_info.rank,
                matched_fields: match_info.matched_fields,
                missing_terms: match_info.missing_terms,
                snippets: match_info.snippets,
                graph_context: SearchGraphContext {
                    actor_ids: actor_ids_for_decision,
                    delegated_by,
                    supersedes_decision_ids,
                    superseded_by_decision_ids,
                    option_ids,
                    evidence_ids,
                    hypotheses,
                    grounding_state,
                    matched_nodes: match_info.matched_nodes,
                },
                scope: None,
                also_recorded_as: Vec::new(),
            },
        });
    }

    Ok(admit_below_bar(
        scored,
        holdings,
        searched,
        terms.terms.len(),
    ))
}

/// What `admit_below_bar` knows of a matched decision.
struct Holding {
    /// `SearchMatchInfo::below_bar`.
    below_bar: bool,
    /// `SearchMatchInfo::held_words`.
    held: BTreeSet<usize>,
    /// `SearchMatchInfo::headline_words`.
    headline: BTreeSet<usize>,
}

/// Fewer decisions than this say too little about which words are rare, so a lone word is trusted
/// when it is the one thing a decision's own title or topic keys name (see `admit_below_bar`).
const SMALL_LEDGER: usize = 16;
/// The words a decision shares with the question must be, together, this many times rarer than a
/// word held by one decision of the `searched + 1` (natural log of the odds), see `admit_below_bar`.
const SHARED_WORDS_RARITY: f64 = 1.25;
/// ... and carry at least this share of the weight of all the question's words.
const SHARED_WORDS_SHARE: f64 = 0.2;
/// ... and be at least this many words, or ...
const SHARED_WORDS_MIN: usize = 3;
/// ... this many of them words the decision's own title or topic keys hold.
const SHARED_HEADLINE_WORDS_MIN: usize = 2;
/// At most this many decisions below the bar are added to an answer.
const BELOW_BAR_LIMIT: usize = 3;

/// Which decisions that lack too many of the terms (`SearchMatchInfo::below_bar`) are still
/// answered. A long question names one thing among generic words ("CLI query xpath css text
/// extraction"), and the half-of-the-terms bar loses it. A word weighs the rarer it is among the
/// `searched` decisions: `ln((searched + 1) / (holders + 1/2))`, and a word nobody holds weighs the
/// most of all, so a question mostly about what the ledger lacks never passes. A decision below the
/// bar is answered when
///
/// - it holds the terms `SHARED_WORDS_MIN` or more of them, or `SHARED_HEADLINE_WORDS_MIN` that its
///   title or topic keys hold, as words that together weigh at least `SHARED_WORDS_RARITY` times
///   `ln(searched + 1)` and `SHARED_WORDS_SHARE` of all the terms' weight: that many words few
///   decisions hold do not meet by chance, whereas two that sit in a long rationale do ("load" and
///   "timeout" of a load balancer question), or
/// - the ledger is small (`SMALL_LEDGER`), so counts say little, and a term the decision's title or
///   topic keys hold is held by no other decision: its capturer named it as the subject.
///
/// At most `BELOW_BAR_LIMIT` of them, those holding the most weight, are added; they all lack more
/// terms than any decision at the bar, so they come after those and each is labelled with the
/// terms it lacks. No word list, nothing learned: the counts are the ledger's own.
/// `scored` and `holdings` are in the same order; the terms are the question's, `term_count` of
/// them, and the holdings name them by position.
fn admit_below_bar(
    scored: Vec<ScoredDecisionSearchResult>,
    holdings: Vec<Holding>,
    searched: usize,
    term_count: usize,
) -> Vec<ScoredDecisionSearchResult> {
    if !holdings.iter().any(|holding| holding.below_bar) {
        return scored;
    }
    let mut holders: HashMap<usize, usize> = HashMap::new();
    for holding in &holdings {
        for term in &holding.held {
            *holders.entry(*term).or_default() += 1;
        }
    }
    let ledger = searched as f64 + 1.0;
    let weight = |count: usize| (ledger / (count as f64 + 0.5)).ln();
    let weight_of = |term: usize| holders.get(&term).map_or(0.0, |count| weight(*count));
    let total: f64 = (0..term_count)
        .map(|term| match weight_of(term) {
            held if held > 0.0 => held,
            _ => weight(0),
        })
        .sum();

    let mut below: Vec<(f64, &str, usize)> = Vec::new();
    for (index, (decision, holding)) in scored.iter().zip(&holdings).enumerate() {
        if !holding.below_bar {
            continue;
        }
        let shared: f64 = holding.held.iter().map(|term| weight_of(*term)).sum();
        let pair = (holding.held.len() >= SHARED_WORDS_MIN
            || holding.headline.len() >= SHARED_HEADLINE_WORDS_MIN)
            && shared >= SHARED_WORDS_RARITY * ledger.ln()
            && shared >= SHARED_WORDS_SHARE * total;
        let named = searched <= SMALL_LEDGER
            && holding
                .headline
                .iter()
                .any(|term| holders.get(term) == Some(&1));
        if pair || named {
            below.push((shared, decision.id.as_str(), index));
        }
    }
    below.sort_by(|left, right| right.0.total_cmp(&left.0).then_with(|| left.1.cmp(right.1)));
    let admitted: BTreeSet<usize> = below
        .into_iter()
        .take(BELOW_BAR_LIMIT)
        .map(|(_, _, index)| index)
        .collect();
    scored
        .into_iter()
        .zip(holdings)
        .enumerate()
        .filter_map(|(index, (decision, holding))| {
            (!holding.below_bar || admitted.contains(&index)).then_some(decision)
        })
        .collect()
}

/// One decision candidate for the resolve-by-description primitive (`resolve.rs`): the same
/// deterministic tier ranking `collect_graph_search_results` already computes, plus the
/// `event_origin` recency tiebreak. No new ranking logic — see docs/AGENT_FLUENT_QUERYING.md §1.1.
pub(crate) struct ResolverCandidateRow {
    pub(crate) decision_id: String,
    pub(crate) title: String,
    pub(crate) rank: u8,
    pub(crate) event_origin: i64,
    pub(crate) matched_fields: Vec<String>,
    /// Description terms this decision does not contain; empty for a full match.
    pub(crate) missing_terms: Vec<String>,
    /// The description is negated and this decision's title is not: it is a close candidate even
    /// when it contains every term, and is never resolved to.
    pub(crate) polarity_mismatch: bool,
    /// For a close candidate, how many description terms its title or topic keys contain; 0 for a
    /// full match.
    pub(crate) headline_terms: usize,
    /// How many of the terms it matched only through a stand-in word (`SearchMatchInfo::
    /// stand_in_terms`); 0 for an asker that writes, which never reads stand-ins.
    pub(crate) stand_in_terms: usize,
}

impl ResolverCandidateRow {
    /// Whether this is a close candidate rather than a full match: it lacks some of the terms, or
    /// it is the opposite of what a negated description asked for.
    pub(crate) fn is_close(&self) -> bool {
        !self.missing_terms.is_empty() || self.polarity_mismatch
    }
}

/// Backend-agnostic candidate rows for resolve-by-description: reuses
/// `collect_graph_search_results`'s tier system unmodified (§1.1 of the design), narrowed by an
/// optional topic hint. `description` is required — an empty resolver query is the caller's bug.
/// `related` reads the description as `recall` does (word forms and stand-in words); a verb that
/// writes asks with `false` and matches a word and its inflections only.
pub(crate) fn collect_resolver_candidates(
    graph: &impl GraphView,
    description: &str,
    topic_keys: &[String],
    close: CloseMatch,
    related: bool,
) -> Result<Vec<ResolverCandidateRow>> {
    let question = resolver_question(description);
    let scored = collect_graph_search_results(
        graph,
        Some(description),
        &SearchTerms::resolver(&question.terms, close, question.negated, related),
        topic_keys,
        &[],
        &[],
        &[],
    )?;
    Ok(scored
        .into_iter()
        .map(|scored| ResolverCandidateRow {
            decision_id: scored.id,
            title: scored.result.decision.title,
            rank: scored.rank,
            event_origin: scored.event_origin,
            matched_fields: scored.result.matched_fields,
            missing_terms: scored.result.missing_terms,
            polarity_mismatch: scored.polarity_mismatch,
            headline_terms: scored.headline_terms,
            stand_in_terms: scored.stand_ins,
        })
        .collect())
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

#[derive(Clone, Debug, Default)]
struct SearchMatchInfo {
    rank: u8,
    /// The decision lacks too many terms to pass `CloseMatch::accepts`, but holds some term as a
    /// word: `collect_graph_search_results` keeps it only if the words it holds are rare among the
    /// decisions searched (`admit_below_bar`).
    below_bar: bool,
    /// The terms (by position in the question's terms) some field holds as a whole word or a form
    /// of one, never as part of a longer word; set only for the asker that reads
    /// (`CloseMatch::Half`).
    held_words: BTreeSet<usize>,
    /// The subset of `held_words` the decision's title or topic keys hold.
    headline_words: BTreeSet<usize>,
    /// How many times a field of the title or topic keys holds a term as a word (a term the title
    /// and a topic key both hold counts twice). Among decisions below the bar that hold a word
    /// each, the one whose title and topic keys both name it is the one about it.
    headline_hits: usize,
    /// Query terms no field matched; non-empty only for a close match.
    missing_terms: Vec<String>,
    /// The question is negated and the decision's title is not (see `SearchTerms::negated`).
    polarity_mismatch: bool,
    /// For a close match, how many of the terms the decision's title or topic keys contain; 0 for
    /// a full match, which `rank` already orders. A decision about a thing says it in its
    /// headline, so among close matches lacking the same number of terms, the one whose headline
    /// carries more of the words it did match is the likelier answer than one that only holds
    /// them somewhere in a long rationale or in evidence.
    headline_terms: usize,
    /// How many of the matched terms the decision has only as a stand-in word (`WORD_GROUPS`),
    /// never as the term or a form of it. A decision that has the word asked for is a better
    /// answer than one that has a synonym of it, so among decisions lacking the same number of
    /// terms the one that leans on fewer stand-ins comes first.
    stand_in_terms: usize,
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

/// When a decision that lacks some of a query's terms is still returned, and with how few.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CloseMatch {
    /// Every term must match: `search`, which keeps literal matching.
    Never,
    /// More than half of the terms, and at least two. A resolver's close candidates feed the
    /// fluent follow-up verbs that can write (`disagree`, `supersede`), so one shared word is
    /// never enough.
    Majority,
    /// At least half of the terms. `recall` only reads and labels every item with what it lacks,
    /// so a question naming two things ("sign-in and pricing") still finds a decision about
    /// either, after the ones about both. The fluent verbs that only read (`why`, `verify`)
    /// resolve with the same bar, so they name what `recall` just named.
    Half,
}

impl CloseMatch {
    /// Whether a decision matching `matched` of `total` terms (`matched < total`) is returned.
    fn accepts(self, matched: usize, total: usize) -> bool {
        match self {
            Self::Never => false,
            Self::Majority => matched >= 2 && matched * 2 > total,
            Self::Half => matched >= 1 && matched * 2 >= total,
        }
    }
}

/// The terms a query must find, and how. Every term must match some field (AND) unless `close`
/// admits a decision that lacks some. `literal` terms match as a substring. Fluent terms
/// (`resolver`, `fluent`) also match a field word with the same stem ("move" finds "moves" and
/// "moved"), and when no decision matches every term, a decision matching most of them is
/// returned with the terms it lacks. Related terms (`fluent`, and the resolver of a verb that only
/// reads) also match the other forms of a word ("superseded" finds "supersession") and the words
/// `WORD_GROUPS` lists as standing in for it ("interface" finds "UI").
struct SearchTerms<'a> {
    terms: &'a [String],
    stemmed: bool,
    /// One per term, in the same order, for a request that reads related words; empty otherwise.
    related: Vec<RelatedWord<'a>>,
    close: CloseMatch,
    /// The question was asked negated ("don't adopt Kafka"). The negation is not one of `terms`:
    /// it never makes a decision lack a word. It only marks a decision whose title is not negated
    /// as the opposite of what was asked (`SearchMatchInfo::polarity_mismatch`).
    negated: bool,
}

impl<'a> SearchTerms<'a> {
    fn literal(terms: &'a [String]) -> Self {
        Self {
            terms,
            stemmed: false,
            related: Vec::new(),
            close: CloseMatch::Never,
            negated: false,
        }
    }

    fn resolver(terms: &'a [String], close: CloseMatch, negated: bool, related: bool) -> Self {
        Self {
            terms,
            stemmed: true,
            related: Self::related_words(terms, related),
            close,
            negated,
        }
    }

    fn fluent(terms: &'a [String], negated: bool) -> Self {
        Self {
            terms,
            stemmed: true,
            related: Self::related_words(terms, true),
            close: CloseMatch::Half,
            negated,
        }
    }

    fn related_words(terms: &'a [String], related: bool) -> Vec<RelatedWord<'a>> {
        if related {
            terms.iter().map(|term| RelatedWord::new(term)).collect()
        } else {
            Vec::new()
        }
    }
}

fn evaluate_search_match(
    query: Option<&str>,
    search_terms: &SearchTerms<'_>,
    fields: &[SearchField],
) -> Option<SearchMatchInfo> {
    let terms = search_terms.terms;
    let Some(query) = query else {
        return Some(SearchMatchInfo {
            rank: 4,
            ..SearchMatchInfo::default()
        });
    };

    let mut matched_terms = BTreeSet::new();
    // The matched terms that some field holds as the word itself or a form of it.
    let mut worded_terms: BTreeSet<&String> = BTreeSet::new();
    let mut headline_terms = BTreeSet::new();
    // What `admit_below_bar` needs from the asker that reads: which terms a field holds as a whole
    // word, and which of those the title or topic keys hold.
    let track_words = search_terms.close == CloseMatch::Half;
    let mut held_words = BTreeSet::new();
    let mut headline_words = BTreeSet::new();
    let mut headline_hits = 0_usize;
    let mut matched_fields = BTreeSet::new();
    let mut snippets = Vec::new();
    let mut matched_nodes = BTreeSet::new();
    // The question a decision answers, asked back as recorded, names that decision as an exact
    // title does.
    let quotes_question = exact_question_match(query, fields);
    let mut rank = if quotes_question || exact_id_or_title_match(query, fields) {
        0
    } else {
        u8::MAX
    };

    for field in fields {
        let value_lower = field.value.to_ascii_lowercase();
        let mut field_stems: Option<BTreeSet<&str>> = None;
        let mut field_matched = false;
        let in_headline = matches!(field.field.as_str(), "decision.title" | "decision.topic");
        for (index, term) in terms.iter().enumerate() {
            let found = if value_lower.contains(term) {
                Some(WordMatch::Word)
            } else if let Some(related) = search_terms.related.get(index) {
                related.find_in(&value_lower)
            } else if search_terms.stemmed
                && field_stems
                    .get_or_insert_with(|| word_stems(&value_lower))
                    .contains(stem(term))
            {
                Some(WordMatch::Word)
            } else {
                None
            };
            if let Some(how) = found {
                matched_terms.insert(term.clone());
                if how == WordMatch::Word {
                    worded_terms.insert(term);
                }
                if in_headline {
                    headline_terms.insert(term.clone());
                }
                if track_words
                    && holds_whole_word(search_terms.related.get(index), &value_lower, term)
                {
                    held_words.insert(index);
                    if in_headline {
                        headline_words.insert(index);
                        headline_hits += 1;
                    }
                }
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

    let missing_terms: Vec<String> = terms
        .iter()
        .filter(|term| !matched_terms.contains(*term))
        .cloned()
        .collect();
    let mut below_bar = false;
    if !missing_terms.is_empty()
        && !search_terms
            .close
            .accepts(terms.len() - missing_terms.len(), terms.len())
    {
        // Too few of the words for the bar. A decision holding one of them as a word is not
        // dropped yet: whether the words it holds are rare is known only across all decisions
        // (`admit_below_bar`).
        if track_words && !held_words.is_empty() {
            below_bar = true;
        } else {
            return None;
        }
    }

    // The title is the sentence that states what was decided; a rationale says "not" for a dozen
    // reasons that leave the decision itself positive. A question quoted as recorded is the one
    // the decision answers, whatever its words ("... or no Jev?", "... without judging"): its
    // polarity is the question's own, never the opposite of the decision.
    let polarity_mismatch = search_terms.negated
        && !quotes_question
        && !fields
            .iter()
            .any(|field| field.field == "decision.title" && is_negated_text(&field.value));

    Some(SearchMatchInfo {
        rank,
        below_bar,
        held_words,
        headline_words,
        headline_hits,
        polarity_mismatch,
        headline_terms: if missing_terms.is_empty() {
            0
        } else {
            headline_terms.len()
        },
        stand_in_terms: matched_terms.len() - worded_terms.len(),
        missing_terms,
        matched_fields: matched_fields.into_iter().collect(),
        snippets,
        matched_nodes: matched_nodes.into_iter().collect(),
    })
}

/// Whether `text` (lowercase) holds `term` as a word or a form of one, never as a part of a longer
/// word: "off" is not in "offset". `related` is the term's forms when the asker reads them.
fn holds_whole_word(related: Option<&RelatedWord<'_>>, text: &str, term: &str) -> bool {
    match related {
        Some(related) => related.find_in(text) == Some(WordMatch::Word),
        None => word_stems(text).contains(stem(term)),
    }
}

fn exact_id_or_title_match(query: &str, fields: &[SearchField]) -> bool {
    let query = query.to_ascii_lowercase();
    fields.iter().any(|field| {
        matches!(field.field.as_str(), "decision.id" | "decision.title")
            && field.value.to_ascii_lowercase() == query
    })
}

/// Whether the query is the question the decision answers, as recorded: matched without case and
/// without a closing "?", since a person asking it back rarely types them the way it was recorded.
fn exact_question_match(query: &str, fields: &[SearchField]) -> bool {
    let asked = without_closing_marks(query);
    !asked.is_empty()
        && fields.iter().any(|field| {
            field.field == "decision.question"
                && without_closing_marks(&field.value).eq_ignore_ascii_case(asked)
        })
}

/// `text` without the whitespace and question marks that close it.
fn without_closing_marks(text: &str) -> &str {
    text.trim_end_matches(|character: char| character == '?' || character.is_whitespace())
        .trim_start()
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
