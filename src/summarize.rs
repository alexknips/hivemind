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
use std::time::Instant;

use serde::Serialize;

use crate::projector::{GraphParams, GraphValue, GraphView};
use crate::queries::{get_decision, get_supersession_chain, DecisionView, QueryResponse};
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
    let id = &ids[0];
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

    let cited: Vec<String> = views.iter().map(|v| v.id.clone()).collect();

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
    let anchor_id = &ids[0];
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

    let cited: Vec<String> = views.iter().map(|v| v.id.clone()).collect();
    let oldest_title = views.first().map(|v| v.title.as_str()).unwrap_or("?");
    let latest_status = views
        .last()
        .map(|v| v.status)
        .unwrap_or(crate::queries::DecisionStatus::Proposed);

    let mut lines: Vec<String> = Vec::new();
    lines.push(format!(
        "Supersession chain for '{}' ({} steps):",
        oldest_title,
        views.len()
    ));
    lines.push(String::new());
    for (i, view) in views.iter().enumerate() {
        let option_labels = fetch_option_labels(graph, &view.option_ids)?;
        let chosen = chosen_label(&option_labels, view.chosen_option_id.as_deref());
        let rationale_short = trim_rationale(&view.rationale, RATIONALE_TRIM_CHARS);
        let chosen_part = chosen.map(|c| format!("; chose: {c}")).unwrap_or_default();
        lines.push(format!(
            "{}. {} ({:?}): {} — {}{}",
            i + 1,
            view.id,
            view.status,
            view.title,
            rationale_short,
            chosen_part,
        ));
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
                Some(GraphValue::String(s)) => s.clone(),
                _ => return None,
            };
            let label = row
                .get("label")
                .and_then(|v| {
                    if let GraphValue::String(s) = v {
                        Some(s.clone())
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| id.clone());
            Some((id, label))
        })
        .collect();
    Ok(option_ids
        .iter()
        .filter_map(|id| label_map.remove(id).map(|label| (id.clone(), label)))
        .collect())
}

fn chosen_label<'a>(
    option_labels: &'a [(String, String)],
    chosen_id: Option<&str>,
) -> Option<&'a str> {
    let chosen_id = chosen_id?;
    option_labels
        .iter()
        .find(|(id, _)| id == chosen_id)
        .map(|(_, label)| label.as_str())
}

fn trim_rationale(rationale: &str, max_chars: usize) -> String {
    if rationale.chars().count() <= max_chars {
        rationale.to_owned()
    } else {
        let trimmed: String = rationale.chars().take(max_chars).collect();
        format!("{trimmed}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
            option_labels
                .iter()
                .position(|l| *l == c)
                .map(|i| option_ids[i].clone())
        });
        commands
            .propose_decision(
                actor,
                title,
                rationale,
                &topic_keys,
                &option_ids,
                chosen_id.as_deref(),
                &[],
                &[],
            )
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
        let outcome = commands
            .supersede(
                "test-actor",
                &old_id,
                "New Decision",
                "New rationale supersedes old",
                &["api".to_string()],
                &[],
                None,
                &[],
                &[],
            )
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
        assert!(
            summary.contains("Supersession chain"),
            "chain header present" // ubs:ignore: test-only assertion
        );
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
