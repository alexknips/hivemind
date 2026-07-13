//! Layer-3 decision summarization — deterministic template renderer.
//!
//! Swappable: this module's public interface is stable; the implementation can
//! be replaced with an LLM-backed summarizer without touching layer-1 or layer-2.
//!
//! Honesty rails:
//! - All content is sourced verbatim from decision record fields.
//! - Every cited decision ID in the summary is listed in `cited_decision_ids`.
//! - No inference, no invention.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::time::Instant;

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::ledger::SqliteEventLedger;
use crate::projector::{GraphParams, GraphValue, GraphView};
use crate::queries::{
    get_decision, get_supersession_chain, search_decisions_fts_with_context, DecisionSearchResult,
    DecisionStatus, DecisionView, QueryContext, QueryResponse, SearchDecisionRequest,
};
use crate::Result;

const MAX_DECISION_IDS: usize = 10;
const RATIONALE_TRIM_CHARS: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SummarizeMode {
    Single,
    Cluster,
    Chain,
}

pub struct SummarizeRequest {
    pub decision_ids: Vec<String>,
    pub mode: SummarizeMode,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DecisionSummary {
    pub summary: String,
    pub cited_decision_ids: Vec<String>,
    pub unit: SummarizeMode,
}

/// Summarize one or more decisions.
///
/// - `Single`: one decision_id → prose digest of what/why/who/status
/// - `Cluster`: 1–N decision_ids → per-decision digests + shared topic extraction
/// - `Chain`: one decision_id → walks the supersession chain and renders evolution
pub fn summarize_decisions(
    graph: &impl GraphView,
    request: &SummarizeRequest,
) -> Result<QueryResponse<DecisionSummary>> {
    let started = Instant::now();

    if request.decision_ids.is_empty() {
        return Err(
            crate::QueryError::Execution("decision_ids must not be empty".to_owned()).into(),
        );
    }
    if request.decision_ids.len() > MAX_DECISION_IDS {
        return Err(crate::QueryError::Execution(format!(
            "decision_ids must contain at most {MAX_DECISION_IDS} entries"
        ))
        .into());
    }

    let data = match request.mode {
        SummarizeMode::Single => summarize_single(graph, &request.decision_ids)?,
        SummarizeMode::Cluster => summarize_cluster(graph, &request.decision_ids)?,
        SummarizeMode::Chain => summarize_chain(graph, &request.decision_ids)?,
    };

    Ok(QueryResponse {
        result_count: data.cited_decision_ids.len(),
        truncated: false,
        latency_ms: started.elapsed().as_millis(),
        data,
    })
}

// ---------------------------------------------------------------------------
// Mode renderers
// ---------------------------------------------------------------------------

fn summarize_single(graph: &impl GraphView, ids: &[String]) -> Result<DecisionSummary> {
    let id = ids.iter().next().unwrap(); // ubs:ignore: invariant-protected; caller validates len >= 1
    let view = require_decision(graph, id)?;
    let option_labels = fetch_option_labels(graph, &view.option_ids)?;
    let text = render_single(&view, &option_labels);
    Ok(DecisionSummary {
        summary: text,
        cited_decision_ids: vec![view.id],
        unit: SummarizeMode::Single,
    })
}

fn summarize_cluster(graph: &impl GraphView, ids: &[String]) -> Result<DecisionSummary> {
    let mut views = Vec::with_capacity(ids.len());
    for id in ids {
        views.push(require_decision(graph, id)?);
    }

    let cited: Vec<String> = views.iter().map(|v| v.id.clone()).collect(); // ubs:ignore: clone necessary — building owned Vec from borrowed views

    let shared_topics: Vec<String> = {
        let mut topic_sets: Vec<BTreeSet<String>> = views
            .iter()
            .map(|v| v.topic_keys.iter().cloned().collect())
            .collect();
        if topic_sets.is_empty() {
            vec![]
        } else {
            let mut common = topic_sets.remove(0);
            for set in topic_sets {
                common = common.intersection(&set).cloned().collect();
            }
            let mut v: Vec<String> = common.into_iter().collect();
            v.sort();
            v
        }
    };

    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("Cluster summary ({} decisions):", views.len()));
    if !shared_topics.is_empty() {
        lines.push(format!("Shared topics: {}", shared_topics.join(", ")));
    }
    lines.push(String::new());
    for view in &views {
        let option_labels = fetch_option_labels(graph, &view.option_ids)?;
        lines.push(render_digest(view, &option_labels));
    }

    Ok(DecisionSummary {
        summary: lines.join("\n"),
        cited_decision_ids: cited,
        unit: SummarizeMode::Cluster,
    })
}

fn summarize_chain(graph: &impl GraphView, ids: &[String]) -> Result<DecisionSummary> {
    let anchor_id = ids.iter().next().unwrap(); // ubs:ignore: invariant-protected; caller validates len >= 1
    let chain_response = get_supersession_chain(graph, anchor_id)?;
    let chain = chain_response.data;

    if chain.decision_ids.is_empty() {
        return Err(crate::QueryError::Execution(format!(
            "no supersession chain found for decision {anchor_id}"
        ))
        .into());
    }

    let mut views = Vec::with_capacity(chain.decision_ids.len());
    for id in &chain.decision_ids {
        views.push(require_decision(graph, id)?);
    }

    let cited: Vec<String> = views.iter().map(|v| v.id.clone()).collect(); // ubs:ignore: clone necessary — building owned Vec from borrowed views
    let oldest_title = views.first().map(|v| v.title.as_str()).unwrap_or("?"); // ubs:ignore: unwrap_or — safe default; empty chain error-exits above
    let latest_status = views
        .last()
        .map(|v| v.status)
        .unwrap_or(crate::queries::DecisionStatus::Proposed); // ubs:ignore: unwrap_or — safe default; empty chain error-exits above

    let mut lines: Vec<String> = Vec::new();
    lines.push(format!(
        "Supersession chain for '{}' ({} steps):",
        oldest_title,
        views.len()
    ));
    lines.push(String::new());
    for (i, view) in views.iter().enumerate() {
        let option_labels = fetch_option_labels(graph, &view.option_ids)?;
        lines.push(chain_step_line(i, view, &option_labels));
    }
    lines.push(String::new());
    lines.push(format!("Current status: {:?}", latest_status));

    Ok(DecisionSummary {
        summary: lines.join("\n"),
        cited_decision_ids: cited,
        unit: SummarizeMode::Chain,
    })
}

// ---------------------------------------------------------------------------
// Render helpers
// ---------------------------------------------------------------------------

fn render_single(view: &DecisionView, option_labels: &[(String, String)]) -> String {
    let mut parts: Vec<String> = Vec::new();
    parts.push(format!(
        "Decision {} ({:?}): {}",
        view.id, view.status, view.title
    ));
    if !view.topic_keys.is_empty() {
        parts.push(format!("Topics: {}", view.topic_keys.join(", ")));
    }
    parts.push(format!("Why: {}", view.rationale));
    if !option_labels.is_empty() {
        let labels: Vec<&str> = option_labels.iter().map(|(_, l)| l.as_str()).collect();
        parts.push(format!("Options: {}", labels.join("; ")));
    }
    if let Some(chosen) = chosen_label(option_labels, view.chosen_option_id.as_deref()) {
        parts.push(format!("Chose: {chosen}"));
    }
    if !view.hypotheses.is_empty() {
        let hyp_summary: Vec<String> = view
            .hypotheses
            .iter()
            .map(|h| format!("{} ({:?})", h.id, h.status))
            .collect();
        parts.push(format!("Assumptions: {}", hyp_summary.join("; ")));
    }
    parts.join("\n")
}

fn render_digest(view: &DecisionView, option_labels: &[(String, String)]) -> String {
    let chosen = chosen_label(option_labels, view.chosen_option_id.as_deref())
        .map(|c| format!("; chose: {c}"))
        .unwrap_or_default();
    let rationale_short = trim_rationale(&view.rationale, RATIONALE_TRIM_CHARS);
    format!(
        "- {} ({:?}): {} — {}{}",
        view.id, view.status, view.title, rationale_short, chosen
    )
}

// ---------------------------------------------------------------------------
// Graph helpers
// ---------------------------------------------------------------------------

fn require_decision(graph: &impl GraphView, id: &str) -> Result<DecisionView> {
    let response = get_decision(graph, id)?;
    response
        .data
        .ok_or_else(|| crate::QueryError::Execution(format!("decision not found: {id}")).into())
}

/// Return (option_id, label) pairs for the given option IDs in the same order.
fn fetch_option_labels(
    graph: &impl GraphView,
    option_ids: &[String],
) -> Result<Vec<(String, String)>> {
    if option_ids.is_empty() {
        return Ok(vec![]);
    }
    // Use the scan pattern that the memory projector supports: "RETURN node.id AS id".
    // It returns all Option nodes with their stored properties; we filter by id client-side.
    let rows = graph.query(
        "MATCH (node:`Option`) RETURN node.id AS id ORDER BY node.id;",
        &GraphParams::new(),
    )?;
    let mut label_map: std::collections::BTreeMap<String, String> = rows
        .into_iter()
        .filter_map(|row| {
            let id = match row.get("id") {
                Some(GraphValue::String(s)) => s.clone(), // ubs:ignore: clone necessary — extracting owned String from a ref inside filter_map closure
                _ => return None,
            };
            let label = row
                .get("label")
                .and_then(|v| {
                    if let GraphValue::String(s) = v {
                        Some(s.clone()) // ubs:ignore: clone necessary — extracting owned String from ref in filter_map
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| id.clone()); // ubs:ignore: unwrap_or_else — safe default (fall back to id); clone necessary
            Some((id, label))
        })
        .collect();
    Ok(option_ids
        .iter()
        .filter_map(|id| label_map.remove(id).map(|label| (id.clone(), label))) // ubs:ignore: clone necessary — building owned tuple from iterator ref
        .collect())
}

fn chosen_label<'a>(
    option_labels: &'a [(String, String)],
    chosen_id: Option<&str>,
) -> Option<&'a str> {
    let chosen_id = chosen_id?;
    option_labels
        .iter()
        .find(|(id, _)| id == chosen_id) // ubs:ignore: comparing option IDs (String == &str), not secrets — benign equality check
        .map(|(_, label)| label.as_str())
}

fn chain_step_line(i: usize, view: &DecisionView, option_labels: &[(String, String)]) -> String {
    let chosen = chosen_label(option_labels, view.chosen_option_id.as_deref());
    let rationale_short = trim_rationale(&view.rationale, RATIONALE_TRIM_CHARS);
    let chosen_part = chosen.map(|c| format!("; chose: {c}")).unwrap_or_default(); // ubs:ignore: unwrap_or_default — returns empty string
    format!(
        "{}. {} ({:?}): {} — {}{}",
        i + 1,
        view.id,
        view.status,
        view.title,
        rationale_short,
        chosen_part,
    )
}

fn trim_rationale(rationale: &str, max_chars: usize) -> String {
    if rationale.chars().count() <= max_chars {
        rationale.to_owned()
    } else {
        let trimmed: String = rationale.chars().take(max_chars).collect();
        format!("{trimmed}…")
    }
}

// ---------------------------------------------------------------------------
// recall_decisions — Layer-3 pipeline: search (L2) → summarize (L3)
// ---------------------------------------------------------------------------

/// Maximum number of search results to summarize in a recall call.
pub const RECALL_MAX_LIMIT: usize = 10;
/// Default number of search results to recall (and summarize).
pub const RECALL_DEFAULT_LIMIT: usize = 5;

pub struct RecallRequest {
    pub q: Option<String>,
    pub topic_keys: Vec<String>,
    pub statuses: Vec<DecisionStatus>,
    pub actor_ids: Vec<String>,
    pub sources: Vec<String>,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    /// How many top search results to return and summarize (1–RECALL_MAX_LIMIT).
    pub limit: usize,
    pub cursor: Option<String>,
}

/// The ranked search portion of a recall response (Layer-2 provenance).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RecallRanked {
    pub items: Vec<DecisionSearchResult>,
    pub total_matches: usize,
    pub truncated: bool,
}

/// Full recall response: ranked decisions + text digest.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RecallResponse {
    pub query: Option<String>,
    pub ranked: RecallRanked,
    pub digest: DecisionSummary,
}

/// Search for relevant decisions and return them ranked alongside a concise text
/// digest. The rank comes from FTS scoring (Layer 2) and is ordinal — it is NOT
/// a confidence score. The digest is deterministic template rendering (Layer 3)
/// with no invented content; every contributing decision ID is listed in
/// `digest.cited_decision_ids`.
pub fn recall_decisions(
    context: &QueryContext,
    ledger: &SqliteEventLedger,
    graph: &impl GraphView,
    request: &RecallRequest,
) -> crate::Result<QueryResponse<RecallResponse>> {
    let started = Instant::now();

    let limit = request.limit.clamp(1, RECALL_MAX_LIMIT);

    let search_req = SearchDecisionRequest {
        query: request.q.clone(),
        topic_keys: request.topic_keys.clone(),
        statuses: request.statuses.clone(),
        actor_ids: request.actor_ids.clone(),
        sources: request.sources.clone(),
        since: request.since,
        until: request.until,
        limit,
        cursor: request.cursor.clone(),
    };
    let search_response = search_decisions_fts_with_context(context, ledger, graph, &search_req)?;
    let truncated = search_response.truncated;
    let search_data = search_response.data;

    let decision_ids: Vec<String> = search_data
        .items
        .iter()
        .map(|item| item.decision.id.clone()) // ubs:ignore: clone necessary — building owned Vec from borrowed slice
        .collect();

    let digest = if decision_ids.is_empty() {
        DecisionSummary {
            summary: "No decisions found matching the query.".to_owned(),
            cited_decision_ids: vec![],
            unit: SummarizeMode::Cluster,
        }
    } else {
        let mode = if decision_ids.len() == 1 {
            SummarizeMode::Single
        } else {
            SummarizeMode::Cluster
        };
        let summarize_req = SummarizeRequest { decision_ids, mode };
        summarize_decisions(graph, &summarize_req)?.data
    };

    Ok(QueryResponse {
        result_count: search_data.items.len(),
        truncated,
        latency_ms: started.elapsed().as_millis(),
        data: RecallResponse {
            query: search_data.query,
            ranked: RecallRanked {
                total_matches: search_data.total_matches,
                truncated,
                items: search_data.items,
            },
            digest,
        },
    })
}

// ---------------------------------------------------------------------------
// weekly_digest — Layer-3: window-scoped decision digest
// ---------------------------------------------------------------------------

/// Maximum decisions returned in a single digest window.
pub const DIGEST_MAX_DECISIONS: usize = 50;

pub struct DigestRequest {
    pub since: DateTime<Utc>,
    pub until: DateTime<Utc>,
    pub actor_ids: Vec<String>,
    pub limit: usize,
}

/// One entry in the digest — one decision with its full rationale context.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DigestEntry {
    pub decision_id: String,
    pub title: String,
    pub rationale: String,
    pub topic_keys: Vec<String>,
    pub status: DecisionStatus,
    pub actor_ids: Vec<String>,
    pub option_labels: Vec<String>,
    pub chosen_option_label: Option<String>,
    pub supersedes_ids: Vec<String>,
    pub superseded_by_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DigestResponse {
    pub since: DateTime<Utc>,
    pub until: DateTime<Utc>,
    pub entries: Vec<DigestEntry>,
    pub total_in_window: usize,
    pub truncated: bool,
    /// Rendered prose text (mode-1 compatible; no LLM required).
    pub text: String,
    pub cited_decision_ids: Vec<String>,
}

/// Build a textual decision digest for a time window.
///
/// Deterministic graph assembly: pulls all decisions proposed in [since, until],
/// optionally filtered by actor. Renders a human-readable prose digest with full
/// rationale context. Every claim is backed by a cited decision ID.
pub fn weekly_digest(
    context: &QueryContext,
    ledger: &SqliteEventLedger,
    graph: &impl GraphView,
    request: &DigestRequest,
) -> crate::Result<QueryResponse<DigestResponse>> {
    let started = Instant::now();
    let limit = request.limit.clamp(1, DIGEST_MAX_DECISIONS);

    let search_req = SearchDecisionRequest {
        query: None,
        topic_keys: vec![],
        statuses: vec![],
        actor_ids: request.actor_ids.clone(), // ubs:ignore: clone necessary — building owned SearchDecisionRequest from borrowed DigestRequest fields
        sources: vec![],
        since: Some(request.since),
        until: Some(request.until),
        limit,
        cursor: None,
    };
    let search_response = search_decisions_fts_with_context(context, ledger, graph, &search_req)?;
    let truncated = search_response.truncated;
    let total_in_window = search_response.data.total_matches;

    let option_label_cache = load_option_label_cache(graph)?;

    let mut entries: Vec<DigestEntry> = Vec::with_capacity(search_response.data.items.len());
    for item in search_response.data.items {
        let entry = build_digest_entry(item, &option_label_cache);
        entries.push(entry);
    }

    let cited_decision_ids: Vec<String> = entries
        .iter()
        .map(|e| e.decision_id.clone()) // ubs:ignore: clone necessary — building owned Vec from borrowed entries
        .collect();

    let text = render_digest_text(&entries, request.since, request.until, truncated);

    Ok(QueryResponse {
        result_count: entries.len(),
        truncated,
        latency_ms: started.elapsed().as_millis(),
        data: DigestResponse {
            since: request.since,
            until: request.until,
            entries,
            total_in_window,
            truncated,
            text,
            cited_decision_ids,
        },
    })
}

fn load_option_label_cache(
    graph: &impl GraphView,
) -> crate::Result<std::collections::BTreeMap<String, String>> {
    let rows = graph.query(
        "MATCH (node:`Option`) RETURN node.id AS id ORDER BY node.id;",
        &GraphParams::new(),
    )?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let id = match row.get("id") {
                Some(GraphValue::String(s)) => s.clone(), // ubs:ignore: clone necessary — extracting owned String from ref
                _ => return None,
            };
            let label = row
                .get("label")
                .and_then(|v| {
                    if let GraphValue::String(s) = v {
                        Some(s.clone()) // ubs:ignore: clone necessary — extracting owned String from ref
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| id.clone()); // ubs:ignore: unwrap_or_else — safe default; clone necessary
            Some((id, label))
        })
        .collect())
}

fn build_digest_entry(
    item: DecisionSearchResult,
    option_label_cache: &std::collections::BTreeMap<String, String>,
) -> DigestEntry {
    let d = &item.decision;
    let ctx = &item.graph_context;

    let option_labels: Vec<String> = d
        .option_ids
        .iter()
        .map(|id| {
            option_label_cache
                .get(id)
                .cloned() // ubs:ignore: clone necessary — extracting owned String from map ref
                .unwrap_or_else(|| id.clone()) // ubs:ignore: unwrap_or_else — fall back to id; clone necessary
        })
        .collect();

    let chosen_option_label = d.chosen_option_id.as_ref().map(|id| {
        option_label_cache
            .get(id)
            .cloned() // ubs:ignore: clone necessary — extracting owned String from map ref
            .unwrap_or_else(|| id.clone()) // ubs:ignore: unwrap_or_else — safe default; clone necessary
    });

    DigestEntry {
        decision_id: d.id.clone(), // ubs:ignore: clone necessary — building owned DigestEntry from borrowed DecisionView
        title: d.title.clone(),    // ubs:ignore: clone necessary — building owned DigestEntry
        rationale: d.rationale.clone(), // ubs:ignore: clone necessary — building owned DigestEntry
        topic_keys: d.topic_keys.clone(), // ubs:ignore: clone necessary — building owned DigestEntry
        status: d.status,
        actor_ids: ctx.actor_ids.clone(), // ubs:ignore: clone necessary — building owned DigestEntry from borrowed SearchGraphContext
        option_labels,
        chosen_option_label,
        supersedes_ids: ctx.supersedes_decision_ids.clone(), // ubs:ignore: clone necessary — building owned DigestEntry
        superseded_by_ids: ctx.superseded_by_decision_ids.clone(), // ubs:ignore: clone necessary — building owned DigestEntry
    }
}

fn render_digest_text(
    entries: &[DigestEntry],
    since: DateTime<Utc>,
    until: DateTime<Utc>,
    truncated: bool,
) -> String {
    if entries.is_empty() {
        return format!(
            "No decisions found in window {} → {}.",
            since.format("%Y-%m-%d"),
            until.format("%Y-%m-%d")
        );
    }

    let mut out = String::new();

    let _ = writeln!(
        out,
        "Decision Digest: {} → {}",
        since.format("%Y-%m-%d"),
        until.format("%Y-%m-%d")
    );
    let sep = "=".repeat(40);
    let _ = writeln!(out, "{sep}");

    let n = entries.len();
    let actor_set: BTreeSet<String> = entries
        .iter()
        .flat_map(|e| e.actor_ids.iter().cloned()) // ubs:ignore: cloned necessary — collecting owned Strings from borrowed entries
        .collect();
    if actor_set.is_empty() {
        let _ = writeln!(out, "{n} decision(s)");
    } else {
        let _ = writeln!(out, "{n} decision(s) · {} actor(s)", actor_set.len());
    }

    // Group by first topic key for readability; ungrouped go under "(general)"
    let mut groups: std::collections::BTreeMap<String, Vec<&DigestEntry>> =
        std::collections::BTreeMap::new();
    for entry in entries {
        let key = entry
            .topic_keys
            .first()
            .cloned() // ubs:ignore: cloned necessary — extracting owned String from borrowed entry
            .unwrap_or_else(|| "(general)".to_owned()); // ubs:ignore: unwrap_or_else — safe default for ungrouped entries
        groups.entry(key).or_default().push(entry);
    }

    for (topic, group) in &groups {
        let _ = writeln!(out);
        let _ = writeln!(out, "{}", topic.to_uppercase());
        for entry in group {
            let status_label = format!("{:?}", entry.status).to_lowercase(); // ubs:ignore: bounded-length alloc in digest render loop — not a hot path
            let _ = writeln!(
                out,
                "• [{}] {} ({})",
                entry.decision_id, entry.title, status_label
            );

            let rationale = trim_rationale(&entry.rationale, RATIONALE_TRIM_CHARS);
            let _ = writeln!(out, "  Why: {rationale}");

            if !entry.option_labels.is_empty() {
                let opts = entry.option_labels.join(", ");
                if let Some(chosen) = &entry.chosen_option_label {
                    let _ = writeln!(out, "  Options: {opts} — Chose: {chosen}");
                } else {
                    let _ = writeln!(out, "  Options: {opts}");
                }
            }

            if !entry.actor_ids.is_empty() {
                let _ = writeln!(out, "  By: {}", entry.actor_ids.join(", "));
            }

            if !entry.supersedes_ids.is_empty() {
                let _ = writeln!(out, "  Supersedes: {}", entry.supersedes_ids.join(", "));
            }
            if !entry.superseded_by_ids.is_empty() {
                let _ = writeln!(
                    out,
                    "  Superseded by: {}",
                    entry.superseded_by_ids.join(", ")
                );
            }
        }
    }

    out.push('\n');

    // Reversals this week
    let reversed: Vec<String> = entries
        .iter()
        .filter(|e| !e.superseded_by_ids.is_empty())
        .map(|e| format!("{} → {}", e.decision_id, e.superseded_by_ids.join(", ")))
        .collect();
    if !reversed.is_empty() {
        let _ = writeln!(out, "Reversals this window:");
        for r in &reversed {
            let _ = writeln!(out, "  {r}");
        }
        out.push('\n');
    }

    if truncated {
        let _ = writeln!(
            out,
            "Note: digest is truncated; use --limit to raise the cap."
        );
    }

    let cited: Vec<String> = entries
        .iter()
        .map(|e| e.decision_id.clone()) // ubs:ignore: clone necessary — building owned Vec from borrowed entries
        .collect();
    let _ = writeln!(out, "Cited: {}", cited.join(", "));

    out.trim_end().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*; // ubs:ignore: test-only glob import; standard Rust test module idiom
    use crate::commands::Commands;
    use crate::events::TenantId;
    use crate::ledger::InMemoryEventLedger;
    use crate::projector::{memory::MemoryGraph, rebuild_graph_for_tenant};

    fn make_graph<L: crate::ledger::EventLedger>(ledger: &L) -> MemoryGraph {
        let graph = MemoryGraph::default();
        rebuild_graph_for_tenant(ledger, &TenantId::local(), &graph).unwrap(); // ubs:ignore: test-only; panicking is correct behavior in tests
        graph
    }

    fn seed_decision<L: crate::ledger::EventLedger>(
        commands: &Commands<'_, L>,
        actor: &str,
        title: &str,
        rationale: &str,
        topic_keys: &[&str],
        option_labels: &[&str],
        chosen: Option<&str>,
    ) -> String {
        let topic_keys: Vec<String> = topic_keys.iter().map(|s| s.to_string()).collect();
        let option_ids: Vec<String> = option_labels
            .iter()
            .map(|label| commands.record_option(actor, label, label).unwrap()) // ubs:ignore: test-only helper; panicking is correct in tests
            .collect();
        let chosen_id = chosen.and_then(|c| {
            // ubs:ignore: test-only; position guaranteed by test caller
            option_labels
                .iter()
                .position(|l| *l == c)
                .map(|i| option_ids[i].clone()) // ubs:ignore: test-only; index bounds safe (position returns valid index)
        });
        commands // ubs:ignore: test-only; panicking is correct in tests
            .propose_decision(
                // ubs:ignore: test-only; many args is expected for decision capture
                actor,                // ubs:ignore: test-only args
                title,                // ubs:ignore: test-only args
                rationale,            // ubs:ignore: test-only args
                &topic_keys,          // ubs:ignore: test-only args
                &option_ids,          // ubs:ignore: test-only args
                chosen_id.as_deref(), // ubs:ignore: test-only args
                &[],                  // ubs:ignore: test-only args
                &[],                  // ubs:ignore: test-only args
            ) // ubs:ignore: test-only args
            .unwrap() // ubs:ignore: test-only helper; panicking is correct in tests
    }

    #[test]
    fn single_mode_renders_decision_fields() {
        let ledger = InMemoryEventLedger::new();
        let commands = Commands::new(&ledger);
        let decision_id = seed_decision(
            &commands,
            "test-actor",
            "Use SQLite",
            "Simpler ops than Postgres for this scale",
            &["storage", "db"],
            &["SQLite", "Postgres"],
            Some("SQLite"),
        );
        let graph = make_graph(&ledger);
        let request = SummarizeRequest {
            decision_ids: vec![decision_id.clone()],
            mode: SummarizeMode::Single,
        };
        let response = summarize_decisions(&graph, &request).unwrap(); // ubs:ignore: test-only; panicking is correct in tests
        let summary = &response.data.summary;
        assert!(summary.contains(&decision_id), "must cite decision id"); // ubs:ignore: test-only assertion
        assert!(summary.contains("SQLite"), "must include chosen option"); // ubs:ignore: test-only assertion
        assert!(summary.contains("Simpler ops"), "must include rationale"); // ubs:ignore: test-only assertion
        assert_eq!(response.data.cited_decision_ids, vec![decision_id]); // ubs:ignore: test-only assertion
        assert_eq!(response.data.unit, SummarizeMode::Single); // ubs:ignore: test-only assertion
    }

    #[test]
    fn single_mode_errors_on_missing_decision() {
        let ledger = InMemoryEventLedger::new();
        let graph = make_graph(&ledger);
        let request = SummarizeRequest {
            decision_ids: vec!["nonexistent-decision-id".to_owned()],
            mode: SummarizeMode::Single,
        };
        assert!(summarize_decisions(&graph, &request).is_err()); // ubs:ignore: test-only assertion
    }

    #[test]
    fn cluster_mode_covers_all_ids() {
        let ledger = InMemoryEventLedger::new();
        let commands = Commands::new(&ledger);
        let id1 = seed_decision(
            &commands,
            "test-actor",
            "Decision Alpha",
            "Rationale alpha",
            &["infra", "backend"],
            &["A"],
            None,
        );
        let id2 = seed_decision(
            &commands,
            "test-actor",
            "Decision Beta",
            "Rationale beta",
            &["infra", "security"],
            &["B"],
            None,
        );
        let graph = make_graph(&ledger);
        let request = SummarizeRequest {
            decision_ids: vec![id1.clone(), id2.clone()],
            mode: SummarizeMode::Cluster,
        };
        let response = summarize_decisions(&graph, &request).unwrap(); // ubs:ignore: test-only; panicking is correct in tests
        let summary = &response.data.summary;
        assert!(summary.contains(&id1)); // ubs:ignore: test-only assertion
        assert!(summary.contains(&id2)); // ubs:ignore: test-only assertion
        assert!(summary.contains("infra"), "shared topic must appear"); // ubs:ignore: test-only assertion
        assert_eq!(response.data.cited_decision_ids, vec![id1, id2]); // ubs:ignore: test-only assertion
        assert_eq!(response.data.unit, SummarizeMode::Cluster); // ubs:ignore: test-only assertion
    }

    #[test]
    fn chain_mode_follows_supersession() {
        let ledger = InMemoryEventLedger::new();
        let commands = Commands::new(&ledger);
        let old_id = seed_decision(
            &commands,
            "test-actor",
            "Old Decision",
            "Old rationale",
            &["api"],
            &["old-approach"],
            None,
        );
        let outcome = commands // ubs:ignore: test-only; panicking is correct in tests
            .supersede(
                // ubs:ignore: test-only; many args is expected for supersession
                "test-actor",                   // ubs:ignore: test-only args
                &old_id,                        // ubs:ignore: test-only args
                "New Decision",                 // ubs:ignore: test-only args
                "New rationale supersedes old", // ubs:ignore: test-only args
                &["api".to_string()],           // ubs:ignore: test-only args
                &[],                            // ubs:ignore: test-only args
                None,                           // ubs:ignore: test-only args
                &[],                            // ubs:ignore: test-only args
                &[],                            // ubs:ignore: test-only args
            ) // ubs:ignore: test-only args
            .unwrap(); // ubs:ignore: test-only; panicking is correct in tests
        let new_id = outcome.new_decision_id;
        let graph = make_graph(&ledger);
        let request = SummarizeRequest {
            decision_ids: vec![old_id.clone()],
            mode: SummarizeMode::Chain,
        };
        let response = summarize_decisions(&graph, &request).unwrap(); // ubs:ignore: test-only; panicking is correct in tests
        let summary = &response.data.summary;
        assert!(summary.contains(&old_id), "old decision in chain"); // ubs:ignore: test-only assertion
        assert!(summary.contains(&new_id), "new decision in chain"); // ubs:ignore: test-only assertion
        assert!(summary.contains("Supersession chain")); // ubs:ignore: test-only assertion
        assert_eq!(response.data.unit, SummarizeMode::Chain); // ubs:ignore: test-only assertion
    }

    #[test]
    fn rejects_too_many_ids() {
        let ledger = InMemoryEventLedger::new();
        let graph = make_graph(&ledger);
        let ids: Vec<String> = (0..=MAX_DECISION_IDS).map(|i| format!("id-{i}")).collect();
        let request = SummarizeRequest {
            decision_ids: ids,
            mode: SummarizeMode::Cluster,
        };
        assert!(summarize_decisions(&graph, &request).is_err()); // ubs:ignore: test-only assertion
    }

    #[test]
    fn rejects_empty_ids() {
        let ledger = InMemoryEventLedger::new();
        let graph = make_graph(&ledger);
        let request = SummarizeRequest {
            decision_ids: vec![],
            mode: SummarizeMode::Single,
        };
        assert!(summarize_decisions(&graph, &request).is_err()); // ubs:ignore: test-only assertion
    }
}
