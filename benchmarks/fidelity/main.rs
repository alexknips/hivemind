//! Capture-fidelity evaluator (Phase 1).
//!
//! Reads benchmarks/fidelity/corpus.yaml, runs the real Haiku classifier on
//! each case, projects CaptureItems to typed nodes+edges, diffs against the
//! hand-authored gold, and prints per-kind P/R/F1 + a macro-F1 headline.
//!
//! Requires ANTHROPIC_API_KEY. Exits 0 even when scores are low (run as a
//! scorecard tool, not a pass/fail gate).
//!
//! Usage:
//!   cargo run --bin fidelity-eval [-- --corpus path/to/corpus.yaml]

use std::collections::HashMap;
use std::path::PathBuf;

use serde::Deserialize;

// --------------------------------------------------------------------------
// Corpus types
// --------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct Corpus {
    cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
struct Case {
    id: String,
    input: String,
    expected: Expected,
}

#[derive(Debug, Deserialize)]
struct Expected {
    #[serde(default)]
    nodes: Vec<GoldNode>,
    #[serde(default)]
    edges: Vec<GoldEdge>,
}

// `chosen`, `status`, `confidence` are parsed from corpus YAML but not scored
// in Phase 1; kept for forward-compat (Phase 2 adds confidence scoring).
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct GoldNode {
    kind: String,
    key: String,
    text: String,
    #[serde(default)]
    chosen: Option<bool>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    confidence: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GoldEdge {
    kind: String,
    from: String,
    to: String,
}

// --------------------------------------------------------------------------
// Scored graph — normalized representation for diff
// --------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ScoredNode {
    kind: String,
    text: String, // normalized
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ScoredEdge {
    kind: String, // canonical uppercase
    from_kind: String,
    from_text: String, // normalized
    to_kind: String,
    to_text: String, // normalized
}

// --------------------------------------------------------------------------
// Text normalization
// --------------------------------------------------------------------------

fn normalize(s: &str) -> String {
    let lower = s.to_lowercase();
    // strip non-alphanumeric-non-space, collapse whitespace
    let stripped: String = lower
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' {
                c
            } else {
                ' '
            }
        })
        .collect();
    stripped.split_whitespace().collect::<Vec<_>>().join(" ")
}

// --------------------------------------------------------------------------
// Edge kind canonicalization
// --------------------------------------------------------------------------

fn canonical_edge_kind(raw: &str) -> String {
    // Corpus uses both UPPER_SNAKE and CamelCase; canonicalize to UPPER_SNAKE.
    match raw {
        "HAS_OPTION" | "HasOption" => "HAS_OPTION",
        "CHOSE" | "Chose" => "CHOSE",
        "BASED_ON" | "BasedOn" => "BASED_ON",
        "SUPERSEDES" | "Supersedes" => "SUPERSEDES",
        "ASSUMES" | "Assumes" => "ASSUMES",
        "SUPPORTS" | "Supports" => "SUPPORTS",
        "REFUTES" | "Refutes" => "REFUTES",
        "ProposedBy" | "PROPOSED_BY" => "PROPOSED_BY",
        "AcceptedBy" | "ACCEPTED_BY" => "ACCEPTED_BY",
        "RejectedBy" | "REJECTED_BY" => "REJECTED_BY",
        "DecisionRequestedBy" | "DECISION_REQUESTED_BY" => "DECISION_REQUESTED_BY",
        "BlockerForDecision" | "BLOCKER_FOR_DECISION" => "BLOCKER_FOR_DECISION",
        "BlockerRequiredOwner" | "BLOCKER_REQUIRED_OWNER" => "BLOCKER_REQUIRED_OWNER",
        "BlockedActor" | "BLOCKED_ACTOR" => "BLOCKED_ACTOR",
        "DecisionRequestForDecision" | "DECISION_REQUEST_FOR_DECISION" => {
            "DECISION_REQUEST_FOR_DECISION"
        }
        other => other,
    }
    .to_owned()
}

// --------------------------------------------------------------------------
// Gold graph projection
// --------------------------------------------------------------------------

fn gold_graph(expected: &Expected) -> (Vec<ScoredNode>, Vec<ScoredEdge>) {
    // key -> (kind, normalized_text) map for edge resolution
    let key_map: HashMap<&str, (&str, String)> = expected
        .nodes
        .iter()
        .map(|n| (n.key.as_str(), (n.kind.as_str(), normalize(&n.text))))
        .collect();

    let nodes: Vec<ScoredNode> = expected
        .nodes
        .iter()
        .map(|n| ScoredNode {
            kind: n.kind.clone(),
            text: normalize(&n.text),
        })
        .collect();

    let edges: Vec<ScoredEdge> = expected
        .edges
        .iter()
        .filter_map(|e| {
            let (from_kind, from_text) = key_map.get(e.from.as_str())?;
            let (to_kind, to_text) = key_map.get(e.to.as_str())?;
            Some(ScoredEdge {
                kind: canonical_edge_kind(&e.kind),
                from_kind: from_kind.to_string(),
                from_text: from_text.clone(),
                to_kind: to_kind.to_string(),
                to_text: to_text.clone(),
            })
        })
        .collect();

    (nodes, edges)
}

// --------------------------------------------------------------------------
// Produced graph from CaptureItems
// --------------------------------------------------------------------------

fn produced_graph(
    captures: &[hivemind::events::CaptureItem],
) -> (Vec<ScoredNode>, Vec<ScoredEdge>) {
    let mut nodes: Vec<ScoredNode> = Vec::new();
    let mut edges: Vec<ScoredEdge> = Vec::new();

    for item in captures {
        let kind = capture_kind_to_node_kind(&item.kind);
        let decision_text = normalize(&item.title);
        nodes.push(ScoredNode {
            kind: kind.clone(),
            text: decision_text.clone(),
        });

        if kind == "Decision" || kind == "DecisionRequest" {
            // options → Option nodes + HAS_OPTION edges
            if let Some(options) = &item.options {
                for option_label in options {
                    let option_text = normalize(option_label);
                    nodes.push(ScoredNode {
                        kind: "Option".to_owned(),
                        text: option_text.clone(),
                    });
                    edges.push(ScoredEdge {
                        kind: "HAS_OPTION".to_owned(),
                        from_kind: kind.clone(),
                        from_text: decision_text.clone(),
                        to_kind: "Option".to_owned(),
                        to_text: option_text,
                    });
                }
            }
            // chosen_option → CHOSE edge
            if let Some(chosen) = &item.chosen_option {
                let chosen_text = normalize(chosen);
                edges.push(ScoredEdge {
                    kind: "CHOSE".to_owned(),
                    from_kind: kind.clone(),
                    from_text: decision_text.clone(),
                    to_kind: "Option".to_owned(),
                    to_text: chosen_text,
                });
            }
        }
        // NOTE: BASED_ON, SUPERSEDES, ASSUMES, SUPPORTS, REFUTES, and actor-linkage
        // edges are NOT in the current CaptureItem schema. They appear as recall
        // penalties — this is the honest baseline (Option A).
    }

    (nodes, edges)
}

fn capture_kind_to_node_kind(kind: &str) -> String {
    match kind {
        "decision" => "Decision",
        "evidence" => "Evidence",
        "hypothesis" => "Hypothesis",
        "blocker" => "Blocker",
        "decision-request" => "DecisionRequest",
        "notification" => "Notification",
        _ => kind,
    }
    .to_owned()
}

// --------------------------------------------------------------------------
// Scoring
// --------------------------------------------------------------------------

#[derive(Debug, Default, Clone)]
struct Counts {
    tp: usize,
    fp: usize,
    fn_: usize,
}

impl Counts {
    fn precision(&self) -> f64 {
        let denom = self.tp + self.fp;
        if denom == 0 {
            1.0 // vacuously correct when nothing was produced
        } else {
            self.tp as f64 / denom as f64
        }
    }

    fn recall(&self) -> f64 {
        let denom = self.tp + self.fn_;
        if denom == 0 {
            1.0 // vacuously correct when nothing was expected
        } else {
            self.tp as f64 / denom as f64
        }
    }

    fn f1(&self) -> f64 {
        let p = self.precision();
        let r = self.recall();
        if p + r == 0.0 {
            0.0
        } else {
            2.0 * p * r / (p + r)
        }
    }
}

fn score_nodes(gold: &[ScoredNode], produced: &[ScoredNode]) -> HashMap<String, Counts> {
    let mut by_kind: HashMap<String, Counts> = HashMap::new();

    // For each gold node, check if a produced node matches (kind + text).
    let mut matched_produced: Vec<bool> = vec![false; produced.len()];

    for gn in gold {
        let counts = by_kind.entry(gn.kind.clone()).or_default();
        let matched = produced
            .iter()
            .enumerate()
            .find(|(i, pn)| !matched_produced[*i] && pn.kind == gn.kind && pn.text == gn.text);
        if let Some((i, _)) = matched {
            matched_produced[i] = true;
            counts.tp += 1;
        } else {
            counts.fn_ += 1;
        }
    }

    // FP: produced nodes not matched by any gold node
    for (i, pn) in produced.iter().enumerate() {
        if !matched_produced[i] {
            by_kind.entry(pn.kind.clone()).or_default().fp += 1;
        }
    }

    by_kind
}

fn score_edges(gold: &[ScoredEdge], produced: &[ScoredEdge]) -> HashMap<String, Counts> {
    let mut by_kind: HashMap<String, Counts> = HashMap::new();

    let mut matched_produced: Vec<bool> = vec![false; produced.len()];

    for ge in gold {
        let counts = by_kind.entry(ge.kind.clone()).or_default();
        let matched = produced.iter().enumerate().find(|(i, pe)| {
            !matched_produced[*i]
                && pe.kind == ge.kind
                && pe.from_text == ge.from_text
                && pe.to_text == ge.to_text
        });
        if let Some((i, _)) = matched {
            matched_produced[i] = true;
            counts.tp += 1;
        } else {
            counts.fn_ += 1;
        }
    }

    for (i, pe) in produced.iter().enumerate() {
        if !matched_produced[i] {
            by_kind.entry(pe.kind.clone()).or_default().fp += 1;
        }
    }

    by_kind
}

// --------------------------------------------------------------------------
// Scorecard printing
// --------------------------------------------------------------------------

#[derive(Debug, Default)]
struct CaseResult {
    case_id: String,
    node_counts: HashMap<String, Counts>,
    edge_counts: HashMap<String, Counts>,
}

impl CaseResult {
    fn macro_f1(&self) -> f64 {
        let all_f1s: Vec<f64> = self
            .node_counts
            .values()
            .map(|c| c.f1())
            .chain(self.edge_counts.values().map(|c| c.f1()))
            .collect();
        if all_f1s.is_empty() {
            1.0
        } else {
            all_f1s.iter().sum::<f64>() / all_f1s.len() as f64
        }
    }
}

fn print_case_scorecard(r: &CaseResult) {
    println!("  Case {}", r.case_id);

    let mut node_kinds: Vec<_> = r.node_counts.keys().cloned().collect();
    node_kinds.sort();
    for kind in &node_kinds {
        let c = &r.node_counts[kind];
        println!(
            "    node:{:<20} P={:.2} R={:.2} F1={:.2}  (tp={} fp={} fn={})",
            kind,
            c.precision(),
            c.recall(),
            c.f1(),
            c.tp,
            c.fp,
            c.fn_
        );
    }

    let mut edge_kinds: Vec<_> = r.edge_counts.keys().cloned().collect();
    edge_kinds.sort();
    for kind in &edge_kinds {
        let c = &r.edge_counts[kind];
        println!(
            "    edge:{:<20} P={:.2} R={:.2} F1={:.2}  (tp={} fp={} fn={})",
            kind,
            c.precision(),
            c.recall(),
            c.f1(),
            c.tp,
            c.fp,
            c.fn_
        );
    }

    println!("    macro-F1: {:.2}", r.macro_f1());
    println!();
}

fn aggregate_scorecard(results: &[CaseResult]) {
    println!("=== AGGREGATE SCORECARD ({} cases) ===", results.len());

    // Pool all counts by kind
    let mut node_totals: HashMap<String, Counts> = HashMap::new();
    let mut edge_totals: HashMap<String, Counts> = HashMap::new();
    let mut case_f1s: Vec<f64> = Vec::new();

    for r in results {
        for (kind, c) in &r.node_counts {
            let tot = node_totals.entry(kind.clone()).or_default();
            tot.tp += c.tp;
            tot.fp += c.fp;
            tot.fn_ += c.fn_;
        }
        for (kind, c) in &r.edge_counts {
            let tot = edge_totals.entry(kind.clone()).or_default();
            tot.tp += c.tp;
            tot.fp += c.fp;
            tot.fn_ += c.fn_;
        }
        case_f1s.push(r.macro_f1());
    }

    let mut node_kinds: Vec<_> = node_totals.keys().cloned().collect();
    node_kinds.sort();
    println!("\nNodes:");
    for kind in &node_kinds {
        let c = &node_totals[kind];
        println!(
            "  {:<22} P={:.2} R={:.2} F1={:.2}  (tp={} fp={} fn={})",
            kind,
            c.precision(),
            c.recall(),
            c.f1(),
            c.tp,
            c.fp,
            c.fn_
        );
    }

    let mut edge_kinds: Vec<_> = edge_totals.keys().cloned().collect();
    edge_kinds.sort();
    println!("\nEdges:");
    for kind in &edge_kinds {
        let c = &edge_totals[kind];
        println!(
            "  {:<22} P={:.2} R={:.2} F1={:.2}  (tp={} fp={} fn={})",
            kind,
            c.precision(),
            c.recall(),
            c.f1(),
            c.tp,
            c.fp,
            c.fn_
        );
    }

    let macro_f1 = if case_f1s.is_empty() {
        0.0
    } else {
        case_f1s.iter().sum::<f64>() / case_f1s.len() as f64
    };
    println!("\nMacro-F1 (mean over cases): {:.2}", macro_f1);
}

// --------------------------------------------------------------------------
// Main
// --------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let corpus_path = parse_corpus_arg(&args);

    let api_key = match std::env::var("ANTHROPIC_API_KEY") {
        Ok(k) if !k.trim().is_empty() => k,
        _ => {
            eprintln!("error: ANTHROPIC_API_KEY is not set; the fidelity evaluator requires it");
            std::process::exit(1);
        }
    };

    let corpus_yaml = std::fs::read_to_string(&corpus_path).unwrap_or_else(|e| {
        eprintln!(
            "error: cannot read corpus at {}: {e}",
            corpus_path.display()
        );
        std::process::exit(1);
    });

    let corpus: Corpus = serde_yaml::from_str(&corpus_yaml).unwrap_or_else(|e| {
        eprintln!("error: corpus parse failed: {e}");
        std::process::exit(1);
    });

    println!("HiveMind capture-fidelity evaluator (Phase 1)");
    println!("Corpus: {} cases", corpus.cases.len());
    println!();

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("reqwest client");

    let mut results: Vec<CaseResult> = Vec::new();

    for case in &corpus.cases {
        println!("Running case {} ...", case.id);

        let captures =
            match hivemind::classifier::classify_text(&client, &api_key, &case.input).await {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("  classifier error for {}: {e}", case.id);
                    // Score as empty produced — full recall penalty
                    Vec::new()
                }
            };

        let (gold_nodes, gold_edges) = gold_graph(&case.expected);
        let (prod_nodes, prod_edges) = produced_graph(&captures);

        let node_counts = score_nodes(&gold_nodes, &prod_nodes);
        let edge_counts = score_edges(&gold_edges, &prod_edges);

        let result = CaseResult {
            case_id: case.id.clone(),
            node_counts,
            edge_counts,
        };

        print_case_scorecard(&result);
        results.push(result);
    }

    aggregate_scorecard(&results);
}

fn parse_corpus_arg(args: &[String]) -> PathBuf {
    let mut iter = args.iter().skip(1);
    while let Some(arg) = iter.next() {
        if arg == "--corpus" {
            if let Some(path) = iter.next() {
                return PathBuf::from(path);
            }
        }
    }
    // Default: benchmarks/fidelity/corpus.yaml relative to cwd
    PathBuf::from("benchmarks/fidelity/corpus.yaml")
}

// --------------------------------------------------------------------------
// Unit tests (scoring logic only — no API calls)
// --------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_punctuation_and_lowercases() {
        assert_eq!(normalize("Postgres!"), "postgres");
        assert_eq!(normalize("  Hello, World.  "), "hello world");
        assert_eq!(normalize("Q2 churn up 4pts"), "q2 churn up 4pts");
    }

    #[test]
    fn canonical_edge_kind_maps_camel_and_upper() {
        assert_eq!(canonical_edge_kind("HAS_OPTION"), "HAS_OPTION");
        assert_eq!(canonical_edge_kind("HasOption"), "HAS_OPTION");
        assert_eq!(canonical_edge_kind("ProposedBy"), "PROPOSED_BY");
        assert_eq!(canonical_edge_kind("AcceptedBy"), "ACCEPTED_BY");
        assert_eq!(
            canonical_edge_kind("DecisionRequestedBy"),
            "DECISION_REQUESTED_BY"
        );
        assert_eq!(
            canonical_edge_kind("BlockerForDecision"),
            "BLOCKER_FOR_DECISION"
        );
    }

    #[test]
    fn score_nodes_perfect_match() {
        let gold = vec![
            ScoredNode {
                kind: "Decision".into(),
                text: "use postgres".into(),
            },
            ScoredNode {
                kind: "Option".into(),
                text: "postgres".into(),
            },
        ];
        let produced = gold.clone();
        let counts = score_nodes(&gold, &produced);
        let d = &counts["Decision"];
        assert_eq!((d.tp, d.fp, d.fn_), (1, 0, 0));
        let o = &counts["Option"];
        assert_eq!((o.tp, o.fp, o.fn_), (1, 0, 0));
    }

    #[test]
    fn score_nodes_all_false_positives() {
        let gold: Vec<ScoredNode> = vec![];
        let produced = vec![ScoredNode {
            kind: "Decision".into(),
            text: "invented".into(),
        }];
        let counts = score_nodes(&gold, &produced);
        let d = &counts["Decision"];
        assert_eq!((d.tp, d.fp, d.fn_), (0, 1, 0));
        assert!((d.precision() - 0.0).abs() < 1e-9);
        assert!((d.recall() - 1.0).abs() < 1e-9); // nothing expected → vacuous
    }

    #[test]
    fn score_nodes_all_false_negatives() {
        let gold = vec![ScoredNode {
            kind: "Evidence".into(),
            text: "load test".into(),
        }];
        let produced: Vec<ScoredNode> = vec![];
        let counts = score_nodes(&gold, &produced);
        let e = &counts["Evidence"];
        assert_eq!((e.tp, e.fp, e.fn_), (0, 0, 1));
        assert!((e.precision() - 1.0).abs() < 1e-9); // nothing produced → vacuous
        assert!((e.recall() - 0.0).abs() < 1e-9);
    }

    #[test]
    fn score_edges_perfect() {
        let e = ScoredEdge {
            kind: "HAS_OPTION".into(),
            from_kind: "Decision".into(),
            from_text: "use postgres".into(),
            to_kind: "Option".into(),
            to_text: "postgres".into(),
        };
        let counts = score_edges(std::slice::from_ref(&e), std::slice::from_ref(&e));
        let c = &counts["HAS_OPTION"];
        assert_eq!((c.tp, c.fp, c.fn_), (1, 0, 0));
    }

    #[test]
    fn score_edges_mismatched_text() {
        let gold = vec![ScoredEdge {
            kind: "HAS_OPTION".into(),
            from_kind: "Decision".into(),
            from_text: "use postgres".into(),
            to_kind: "Option".into(),
            to_text: "postgres".into(),
        }];
        let produced = vec![ScoredEdge {
            kind: "HAS_OPTION".into(),
            from_kind: "Decision".into(),
            from_text: "use postgres".into(),
            to_kind: "Option".into(),
            to_text: "mysql".into(), // wrong endpoint
        }];
        let counts = score_edges(&gold, &produced);
        let c = &counts["HAS_OPTION"];
        assert_eq!((c.tp, c.fp, c.fn_), (0, 1, 1));
    }

    #[test]
    fn produced_graph_simple_decision_with_options() {
        let captures = vec![hivemind::events::CaptureItem {
            kind: "decision".into(),
            title: "Use Postgres".into(),
            rationale: "concurrent writes".into(),
            topic_keys: vec!["infra".into()],
            evidence_ids: vec![],
            options: Some(vec!["Postgres".into(), "SQLite".into()]),
            chosen_option: Some("Postgres".into()),
            extraction_confidence: 0.9,
            expressed_confidence: None,
            supersedes_id: None,
            assumes_ids: vec![],
            supports_ids: vec![],
            refutes_ids: vec![],
            actor_id: None,
            accepted_by: None,
            rejected_by: None,
            blocked_actor_id: None,
            decision_id: None,
        }];
        let (nodes, edges) = produced_graph(&captures);
        assert_eq!(nodes.len(), 3); // Decision + 2 Options
        assert!(nodes
            .iter()
            .any(|n| n.kind == "Decision" && n.text == "use postgres"));
        assert!(nodes
            .iter()
            .any(|n| n.kind == "Option" && n.text == "postgres"));
        assert!(nodes
            .iter()
            .any(|n| n.kind == "Option" && n.text == "sqlite"));
        assert_eq!(edges.len(), 3); // 2 HAS_OPTION + 1 CHOSE
        assert!(edges
            .iter()
            .any(|e| e.kind == "CHOSE" && e.to_text == "postgres"));
    }

    #[test]
    fn produced_graph_empty_for_no_captures() {
        let (nodes, edges) = produced_graph(&[]);
        assert!(nodes.is_empty());
        assert!(edges.is_empty());
    }

    #[test]
    fn gold_graph_resolves_edge_keys() {
        let expected = Expected {
            nodes: vec![
                GoldNode {
                    kind: "Decision".into(),
                    key: "d".into(),
                    text: "Use Postgres".into(),
                    chosen: None,
                    status: None,
                    confidence: None,
                },
                GoldNode {
                    kind: "Option".into(),
                    key: "pg".into(),
                    text: "Postgres".into(),
                    chosen: Some(true),
                    status: None,
                    confidence: None,
                },
            ],
            edges: vec![GoldEdge {
                kind: "HAS_OPTION".into(),
                from: "d".into(),
                to: "pg".into(),
            }],
        };
        let (nodes, edges) = gold_graph(&expected);
        assert_eq!(nodes.len(), 2);
        assert_eq!(edges.len(), 1);
        let e = &edges[0];
        assert_eq!(e.kind, "HAS_OPTION");
        assert_eq!(e.from_text, "use postgres");
        assert_eq!(e.to_text, "postgres");
    }

    #[test]
    fn counts_f1_zero_when_tp_is_zero_and_both_exist() {
        let c = Counts {
            tp: 0,
            fp: 1,
            fn_: 1,
        };
        assert!((c.f1() - 0.0).abs() < 1e-9);
    }

    #[test]
    fn counts_precision_vacuous_when_nothing_produced() {
        let c = Counts {
            tp: 0,
            fp: 0,
            fn_: 3,
        };
        assert!((c.precision() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn counts_recall_vacuous_when_nothing_expected() {
        let c = Counts {
            tp: 0,
            fp: 2,
            fn_: 0,
        };
        assert!((c.recall() - 1.0).abs() < 1e-9);
    }
}
