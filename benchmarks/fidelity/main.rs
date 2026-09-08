//! Capture-fidelity evaluator (Phase 1).
//!
//! Reads benchmarks/fidelity/corpus.yaml, runs the real Haiku classifier on
//! each case, projects CaptureItems via the REAL production projector to typed
//! nodes+edges, diffs against the hand-authored gold, and prints per-kind
//! P/R/F1 + a macro-F1 headline.
//!
//! Two LLM backends, selected by `HIVEMIND_EVAL_BACKEND`:
//!   - `anthropic-api` (default when ANTHROPIC_API_KEY is set): metered
//!     Anthropic API, same path as the production classifier.
//!   - `claude-cli` (default when no key is set and `claude` is on PATH):
//!     shells out to the operator's authenticated Claude Code CLI
//!     subscription (`claude -p`), for keyless local runs. Intended for a
//!     human developer running an eval from their own agent session.
//!
//! Both share the exact classifier prompt, schema, and CaptureItem parsing
//! (`hivemind::classifier::{build_prompt, capture_schema, parse_capture_response}`)
//! so scores are comparable across backends.
//!
//! Exits 0 even when scores are low (run as a scorecard tool, not a
//! pass/fail gate).
//!
//! Usage:
//!   cargo run --bin fidelity-eval [-- --corpus path/to/corpus.yaml]
//!   cargo run --bin fidelity-eval -- --ceiling   # schema-ceiling (no LLM)
//!   HIVEMIND_EVAL_BACKEND=claude-cli cargo run --bin fidelity-eval
//!   HIVEMIND_EVAL_BACKEND=anthropic-api cargo run --bin fidelity-eval

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

// --------------------------------------------------------------------------
// Corpus types
// --------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct Corpus {
    cases: Vec<Case>,
    #[serde(default)]
    org_bundles: Vec<OrgDef>,
}

#[derive(Debug, Deserialize)]
struct Case {
    id: String,
    #[serde(default)]
    #[allow(dead_code)]
    project_id: Option<String>,
    input: String,
    expected: Expected,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct OrgDef {
    id: String,
    name: String,
    case_ids: Vec<String>,
    #[serde(default)]
    cross_project_references: Vec<CrossProjectRef>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct CrossProjectRef {
    from_case: String,
    to_case: String,
    from_project: String,
    to_project: String,
    kind: String,
    expected_retrieved: bool,
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
        "PREMISED_ON" | "PremisedOn" => "PREMISED_ON",
        "ASSUMES" | "Assumes" => "PREMISED_ON",
        "SUPPORTS" | "Supports" => "SUPPORTS",
        "REFUTES" | "Refutes" => "REFUTES",
        "ProposedBy" | "PROPOSED_BY" => "PROPOSED_BY",
        "AcceptedBy" | "ACCEPTED_BY" => "ACCEPTED_BY",
        "RejectedBy" | "REJECTED_BY" => "REJECTED_BY",
        "DecisionRequestedBy" | "DECISION_REQUESTED_BY" => "DECISION_REQUESTED_BY",
        "RequestProposedBy" | "REQUEST_PROPOSED_BY" => "REQUEST_PROPOSED_BY",
        "RequestAcceptedBy" | "REQUEST_ACCEPTED_BY" => "REQUEST_ACCEPTED_BY",
        "RequestRejectedBy" | "REQUEST_REJECTED_BY" => "REQUEST_REJECTED_BY",
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
// Produced graph from CaptureItems via the REAL production projector
// --------------------------------------------------------------------------

fn produced_graph(
    id_captures: &[(&str, &hivemind::events::CaptureItem)],
) -> (Vec<ScoredNode>, Vec<ScoredEdge>) {
    match hivemind::projector::project_captures_in_memory(id_captures) {
        Ok((nodes, edges)) => convert_to_scored(nodes, edges),
        Err(e) => {
            eprintln!("  projector error: {e}");
            (Vec::new(), Vec::new())
        }
    }
}

fn convert_to_scored(
    nodes: Vec<(hivemind::projector::NodeKind, String, String)>,
    edges: Vec<(hivemind::projector::RelationKind, String, String)>,
) -> (Vec<ScoredNode>, Vec<ScoredEdge>) {
    // Build id → (kind_str, normalized_text) map for edge endpoint resolution.
    let mut id_map: HashMap<String, (String, String)> = HashMap::new();
    let mut scored_nodes: Vec<ScoredNode> = Vec::new();

    for (kind, id, text) in &nodes {
        let kind_str = kind.table_name().to_owned();
        let norm = normalize(text);
        id_map.insert(id.clone(), (kind_str.clone(), norm.clone()));
        scored_nodes.push(ScoredNode {
            kind: kind_str,
            text: norm,
        });
    }

    let mut scored_edges: Vec<ScoredEdge> = Vec::new();
    for (rel, from_id, to_id) in &edges {
        let kind_str = rel.table_name().to_owned();
        let Some((from_kind, from_text)) = id_map.get(from_id) else {
            continue;
        };
        let Some((to_kind, to_text)) = id_map.get(to_id) else {
            continue;
        };
        scored_edges.push(ScoredEdge {
            kind: kind_str,
            from_kind: from_kind.clone(),
            from_text: from_text.clone(),
            to_kind: to_kind.clone(),
            to_text: to_text.clone(),
        });
    }

    (scored_nodes, scored_edges)
}

// --------------------------------------------------------------------------
// LLM backend selection: metered Anthropic API vs. the operator's Claude
// Code CLI subscription (keyless). See module docs above for the contract.
// --------------------------------------------------------------------------

const CLAUDE_CLI_MODEL: &str = "claude-haiku-4-5-20251001";
const CLAUDE_CLI_TIMEOUT: Duration = Duration::from_secs(90);
const EVAL_BACKEND_ENV: &str = "HIVEMIND_EVAL_BACKEND";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Backend {
    AnthropicApi,
    ClaudeCli,
}

impl Backend {
    fn label(self) -> &'static str {
        match self {
            Backend::AnthropicApi => "anthropic-api",
            Backend::ClaudeCli => "claude-cli",
        }
    }
}

/// True if `claude` resolves on PATH and runs (`claude --version` exits 0).
fn claude_cli_on_path() -> bool {
    Command::new("claude")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Resolve the backend from `HIVEMIND_EVAL_BACKEND`, or auto-detect: prefer
/// `claude-cli` when no ANTHROPIC_API_KEY is set and `claude` is on PATH,
/// otherwise `anthropic-api` (the pre-existing default).
fn resolve_backend() -> Backend {
    match std::env::var(EVAL_BACKEND_ENV) {
        Ok(v) => match v.trim() {
            "claude-cli" => Backend::ClaudeCli,
            "anthropic-api" => Backend::AnthropicApi,
            other => {
                eprintln!(
                    "error: invalid {EVAL_BACKEND_ENV}={other:?}; expected \"claude-cli\" or \"anthropic-api\""
                );
                std::process::exit(1);
            }
        },
        Err(_) => {
            let has_key = std::env::var("ANTHROPIC_API_KEY")
                .map(|k| !k.trim().is_empty())
                .unwrap_or(false);
            if !has_key && claude_cli_on_path() {
                Backend::ClaudeCli
            } else {
                Backend::AnthropicApi
            }
        }
    }
}

/// Run `claude` with stdin piped and stdout/stderr captured, enforcing
/// `timeout` via a watcher thread (no async-process dependency needed: this
/// tool has no other concurrent work while a case is classifying).
fn run_claude_cli(
    args: &[&str],
    prompt: &str,
    timeout: Duration,
) -> Result<std::process::Output, Box<dyn std::error::Error + Send + Sync>> {
    let mut child = Command::new("claude")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to spawn `claude` CLI: {e}"))?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or("no stdin handle for claude subprocess")?;
    let prompt_owned = prompt.to_owned();
    let stdin_writer = std::thread::spawn(move || {
        // Best-effort: a broken pipe (subprocess exited early) is not fatal.
        let _ = stdin.write_all(prompt_owned.as_bytes());
    });

    let pid = child.id();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });

    let output = match rx.recv_timeout(timeout) {
        Ok(result) => result.map_err(|e| format!("claude CLI wait failed: {e}"))?,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            let _ = Command::new("kill").arg("-9").arg(pid.to_string()).status();
            return Err(format!("claude CLI timed out after {timeout:?}").into());
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            return Err("claude CLI wait thread ended without a result".into());
        }
    };
    let _ = stdin_writer.join();
    Ok(output)
}

#[derive(Debug, Deserialize)]
struct ClaudeCliEnvelope {
    #[serde(default)]
    result: Option<String>,
    #[serde(default)]
    is_error: bool,
    #[serde(default)]
    subtype: Option<String>,
}

/// Classify one case's input text via the operator's Claude Code CLI
/// subscription (`claude -p`) instead of the metered Anthropic API. Shares
/// the exact classifier prompt, schema, and CaptureItem parsing with the
/// production classifier so scores are comparable across backends.
fn classify_text_cli(
    input: &str,
) -> Result<Vec<hivemind::events::CaptureItem>, Box<dyn std::error::Error + Send + Sync>> {
    let prompt = hivemind::classifier::build_prompt(input);
    let schema = serde_json::to_string(&hivemind::classifier::capture_schema())?;

    let output = run_claude_cli(
        &[
            "-p",
            "--model",
            CLAUDE_CLI_MODEL,
            "--output-format",
            "json",
            "--json-schema",
            &schema,
            "--tools",
            "",
            "--safe-mode",
        ],
        &prompt,
        CLAUDE_CLI_TIMEOUT,
    )?;

    if !output.status.success() {
        return Err(format!(
            "claude CLI exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }

    let stdout_text = String::from_utf8_lossy(&output.stdout);
    let envelope: ClaudeCliEnvelope = serde_json::from_str(stdout_text.trim()).map_err(|e| {
        format!("failed to parse claude CLI output as JSON: {e}\nstdout: {stdout_text}")
    })?;

    if envelope.is_error || envelope.subtype.as_deref() != Some("success") {
        return Err(format!(
            "claude CLI returned an error result: subtype={:?} stderr={}",
            envelope.subtype,
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }

    let result_text = envelope
        .result
        .ok_or("claude CLI response missing `result` field")?;
    let captures = hivemind::classifier::parse_capture_response(&result_text).map_err(|e| {
        format!(
            "failed to parse CaptureItem schema from claude CLI result: {e}\nresult: {result_text}"
        )
    })?;
    Ok(captures)
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

// Kept for the score_nodes_rubric_exact_matches_score_nodes parity test only
// (Rubric::Exact must reproduce this exactly); the binary's live scoring path
// now always goes through score_nodes_rubric.
#[cfg_attr(not(test), allow(dead_code))]
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

// Kept for the score_edges parity test only; see score_nodes's comment above.
#[cfg_attr(not(test), allow(dead_code))]
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
// Phase 2 rubric: deterministic token-similarity matching
//
// hivemind-g4ft.1: Phase 1 free-text matching is exact-normalized-text
// (see `normalize` above and score_nodes/score_edges). That rubric was
// always documented as provisional — the corpus header has read, since
// hivemind-21zi, "LLM-judge added only in Phase 2 where phrasing varies".
// PRINCIPLES 1/7 rule out an LLM in a scoring path, so Phase 2 here is a
// deterministic similarity measure instead: no model call, fully
// reproducible, and its basis (token Jaccard over normalized text) is
// traceable — no invented confidence per AGENTS.md section 6.
// --------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rubric {
    /// Phase 1: free-text fields must match exactly after normalization.
    Exact,
    /// Phase 2: free-text fields match when token-Jaccard similarity clears
    /// SIMILARITY_THRESHOLD. Still zero LLM calls in the scoring path.
    Similarity,
}

impl Rubric {
    fn parse(raw: &str) -> Self {
        match raw {
            "exact" => Rubric::Exact,
            "similarity" => Rubric::Similarity,
            other => {
                eprintln!(
                    "error: invalid --rubric={other:?}; expected \"exact\" or \"similarity\""
                );
                std::process::exit(1);
            }
        }
    }

    fn label(self) -> &'static str {
        match self {
            Rubric::Exact => "exact",
            Rubric::Similarity => "similarity",
        }
    }
}

/// Threshold picked from manual adjudication of the 265w claude-cli run
/// (hivemind-g4ft.1, 2026-09-08) against the 36-case gold corpus: see the
/// bead notes for the worked true-positive/false-positive pairs that set
/// this boundary.
const SIMILARITY_THRESHOLD: f64 = 0.5;

/// Token-Jaccard similarity, normalizing both inputs first (lowercase,
/// punctuation stripped to spaces — see `normalize`) so callers can pass
/// either raw or already-normalized text. Deterministic, symmetric, no
/// external calls. 1.0 for identical text (including identical empty text),
/// 0.0 when either side is non-empty and they share no tokens.
fn text_similarity(a: &str, b: &str) -> f64 {
    if a == b {
        return 1.0;
    }
    let na = normalize(a);
    let nb = normalize(b);
    if na == nb {
        return 1.0;
    }
    let ta: std::collections::HashSet<&str> = na.split_whitespace().collect();
    let tb: std::collections::HashSet<&str> = nb.split_whitespace().collect();
    if ta.is_empty() || tb.is_empty() {
        return 0.0;
    }
    let intersection = ta.intersection(&tb).count();
    let union = ta.union(&tb).count();
    intersection as f64 / union as f64
}

fn rubric_matches(rubric: Rubric, a: &str, b: &str) -> bool {
    match rubric {
        Rubric::Exact => a == b,
        Rubric::Similarity => a == b || text_similarity(a, b) >= SIMILARITY_THRESHOLD,
    }
}

/// Same TP/FP/FN semantics as score_nodes, but the match predicate is
/// rubric-dependent and, for Similarity, greedily prefers the
/// highest-similarity unmatched candidate (order-independent) rather than
/// the first candidate found.
fn score_nodes_rubric(
    gold: &[ScoredNode],
    produced: &[ScoredNode],
    rubric: Rubric,
) -> HashMap<String, Counts> {
    let mut by_kind: HashMap<String, Counts> = HashMap::new();
    let mut matched_produced: Vec<bool> = vec![false; produced.len()];

    for gn in gold {
        let counts = by_kind.entry(gn.kind.clone()).or_default();
        let best = produced
            .iter()
            .enumerate()
            .filter(|(i, pn)| !matched_produced[*i] && pn.kind == gn.kind)
            .filter(|(_, pn)| rubric_matches(rubric, &gn.text, &pn.text))
            .max_by(|(_, a), (_, b)| {
                text_similarity(&gn.text, &a.text)
                    .partial_cmp(&text_similarity(&gn.text, &b.text))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        if let Some((i, _)) = best {
            matched_produced[i] = true;
            counts.tp += 1;
        } else {
            counts.fn_ += 1;
        }
    }

    for (i, pn) in produced.iter().enumerate() {
        if !matched_produced[i] {
            by_kind.entry(pn.kind.clone()).or_default().fp += 1;
        }
    }

    by_kind
}

/// Same TP/FP/FN semantics as score_edges, rubric-dependent on both endpoint
/// texts (edge kind and endpoint kinds remain exact — only free-text fields
/// get the fuzzy rubric, per the corpus header's matching rule).
fn score_edges_rubric(
    gold: &[ScoredEdge],
    produced: &[ScoredEdge],
    rubric: Rubric,
) -> HashMap<String, Counts> {
    let mut by_kind: HashMap<String, Counts> = HashMap::new();
    let mut matched_produced: Vec<bool> = vec![false; produced.len()];

    let edge_sim = |a: &ScoredEdge, b: &ScoredEdge| -> f64 {
        (text_similarity(&a.from_text, &b.from_text) + text_similarity(&a.to_text, &b.to_text))
            / 2.0
    };

    for ge in gold {
        let counts = by_kind.entry(ge.kind.clone()).or_default();
        let best = produced
            .iter()
            .enumerate()
            .filter(|(i, pe)| !matched_produced[*i] && pe.kind == ge.kind)
            .filter(|(_, pe)| {
                rubric_matches(rubric, &ge.from_text, &pe.from_text)
                    && rubric_matches(rubric, &ge.to_text, &pe.to_text)
            })
            .max_by(|(_, a), (_, b)| {
                edge_sim(ge, a)
                    .partial_cmp(&edge_sim(ge, b))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        if let Some((i, _)) = best {
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

/// Prints, for every unmatched (FN) gold node/edge, its nearest produced
/// candidate of the same kind and the similarity score — the raw material
/// for manual adjudication (hivemind-g4ft.1 acceptance criterion: adjudicate
/// a sample before trusting a rubric change). Independent of which rubric
/// the aggregate scorecard uses.
fn print_mismatches(case_id: &str, gold_nodes: &[ScoredNode], prod_nodes: &[ScoredNode]) {
    for gn in gold_nodes {
        let exact = prod_nodes
            .iter()
            .any(|pn| pn.kind == gn.kind && pn.text == gn.text);
        if exact {
            continue;
        }
        let best = prod_nodes
            .iter()
            .filter(|pn| pn.kind == gn.kind)
            .map(|pn| (pn, text_similarity(&gn.text, &pn.text)))
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        match best {
            Some((pn, sim)) => println!(
                "  MISMATCH[{case_id}] node:{} gold={:?} nearest_produced={:?} sim={:.2}",
                gn.kind, gn.text, pn.text, sim
            ),
            None => println!(
                "  MISMATCH[{case_id}] node:{} gold={:?} nearest_produced=<none of this kind>",
                gn.kind, gn.text
            ),
        }
    }
}

// --------------------------------------------------------------------------
// Scorecard printing
// --------------------------------------------------------------------------

#[derive(Debug, Default)]
struct CaseResult {
    case_id: String,
    node_counts: HashMap<String, Counts>,
    edge_counts: HashMap<String, Counts>,
    /// Provenance: which backend + model produced this case's captures.
    /// "ceiling" / "n/a" in --ceiling mode (no LLM call was made).
    backend: String,
    model: String,
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
    println!("  Case {} [{}/{}]", r.case_id, r.backend, r.model);

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

// --------------------------------------------------------------------------
// Ceiling mode: gold nodes → (id, CaptureItem) pairs (no API call)
// --------------------------------------------------------------------------

// Converts gold expected data to (gold_key, CaptureItem) pairs using the
// REAL production projector. Actor and Option nodes have no standalone
// CaptureItem kind; Actor edges are wired via actor_id/accepted_by/rejected_by
// fields; Option nodes are emitted by project_capture from capture.options.
fn gold_as_captures(expected: &Expected) -> Vec<(String, hivemind::events::CaptureItem)> {
    use hivemind::events::CaptureItem;
    use std::collections::HashMap;

    // key → text for edge resolution
    let key_map: HashMap<&str, &str> = expected
        .nodes
        .iter()
        .map(|n| (n.key.as_str(), n.text.as_str()))
        .collect();

    // Gather edge lists per source/target key for cross-reference population.
    let mut options_map: HashMap<&str, Vec<String>> = HashMap::new();
    let mut chosen_map: HashMap<&str, String> = HashMap::new();
    let mut evidence_ids_map: HashMap<&str, Vec<String>> = HashMap::new();
    let mut supersedes_map: HashMap<&str, String> = HashMap::new();
    let mut premised_on_map: HashMap<&str, Vec<String>> = HashMap::new();
    let mut supports_map: HashMap<&str, Vec<String>> = HashMap::new();
    let mut refutes_map: HashMap<&str, Vec<String>> = HashMap::new();
    // Actor edge maps (CaptureItem field → Actor node keys used as IDs). Both
    // Decision (AcceptedBy/RejectedBy) and DecisionRequest (RequestAcceptedBy/
    // RequestRejectedBy) route into the same accepted_by/rejected_by Vec fields —
    // the projector picks the edge kind from the capture's `kind`.
    let mut accepted_by_map: HashMap<&str, Vec<String>> = HashMap::new();
    let mut rejected_by_map: HashMap<&str, Vec<String>> = HashMap::new();
    let mut dr_actor_id_map: HashMap<&str, String> = HashMap::new();

    for e in &expected.edges {
        match e.kind.as_str() {
            "HAS_OPTION" => {
                if let Some(&opt_text) = key_map.get(e.to.as_str()) {
                    options_map
                        .entry(e.from.as_str())
                        .or_default()
                        .push(opt_text.to_owned());
                }
            }
            "CHOSE" => {
                if let Some(&opt_text) = key_map.get(e.to.as_str()) {
                    chosen_map.insert(e.from.as_str(), opt_text.to_owned());
                }
            }
            "BASED_ON" => {
                // from=Decision, to=Evidence; store the Evidence key as the id.
                evidence_ids_map
                    .entry(e.from.as_str())
                    .or_default()
                    .push(e.to.clone());
            }
            "SUPERSEDES" => {
                // from=new Decision, to=superseded Decision key.
                supersedes_map.insert(e.from.as_str(), e.to.clone());
            }
            "PREMISED_ON" => {
                // from=Decision, to=Hypothesis key.
                premised_on_map
                    .entry(e.from.as_str())
                    .or_default()
                    .push(e.to.clone());
            }
            "SUPPORTS" => {
                // from=Evidence, to=Hypothesis key.
                supports_map
                    .entry(e.from.as_str())
                    .or_default()
                    .push(e.to.clone());
            }
            "REFUTES" => {
                // from=Evidence, to=Hypothesis key.
                refutes_map
                    .entry(e.from.as_str())
                    .or_default()
                    .push(e.to.clone());
            }
            // Actor-linking edges — resolve the target Actor's key to its
            // declared node text (same key_map lookup HAS_OPTION/CHOSE use
            // above), since that text is what actually flows through as the
            // real projector's actor_id and is what gold_graph's ScoredNode
            // compares against. Using the raw key here (as this used to)
            // silently passed for every case where key happened to equal
            // lowercase(text), then produced a wrong-text Actor node instead
            // of the real one for D2/G5, whose gold text carries a
            // descriptive suffix the key doesn't ("Dana (hiring manager)" /
            // "team-platform").
            // Multi-valued: a Decision (or DecisionRequest) may have more than
            // one acceptor/rejecter on record (e.g. G2/G3/G4).
            "AcceptedBy" => {
                if let Some(&text) = key_map.get(e.to.as_str()) {
                    accepted_by_map
                        .entry(e.from.as_str())
                        .or_default()
                        .push(text.to_owned());
                }
            }
            "RejectedBy" => {
                if let Some(&text) = key_map.get(e.to.as_str()) {
                    rejected_by_map
                        .entry(e.from.as_str())
                        .or_default()
                        .push(text.to_owned());
                }
            }
            // DecisionRequestedBy: from=DecisionRequest, to=Actor. Plain open ask,
            // no accepted_by/rejected_by on record (D1/D3).
            "DecisionRequestedBy" => {
                if let Some(&text) = key_map.get(e.to.as_str()) {
                    dr_actor_id_map.insert(e.from.as_str(), text.to_owned());
                }
            }
            // RequestProposedBy: from=DecisionRequest, to=Actor. Contested ask —
            // proposer position, routed the same as DecisionRequestedBy's actor_id;
            // the projector picks RequestProposedBy vs DecisionRequestedBy from
            // whether accepted_by/rejected_by are populated.
            "RequestProposedBy" => {
                if let Some(&text) = key_map.get(e.to.as_str()) {
                    dr_actor_id_map.insert(e.from.as_str(), text.to_owned());
                }
            }
            // RequestAcceptedBy/RequestRejectedBy: from=DecisionRequest, to=Actor.
            // Route into the same Vec fields as Decision's AcceptedBy/RejectedBy.
            "RequestAcceptedBy" => {
                if let Some(&text) = key_map.get(e.to.as_str()) {
                    accepted_by_map
                        .entry(e.from.as_str())
                        .or_default()
                        .push(text.to_owned());
                }
            }
            "RequestRejectedBy" => {
                if let Some(&text) = key_map.get(e.to.as_str()) {
                    rejected_by_map
                        .entry(e.from.as_str())
                        .or_default()
                        .push(text.to_owned());
                }
            }
            _ => {}
        }
    }

    let mut captures: Vec<(String, CaptureItem)> = Vec::new();
    for node in &expected.nodes {
        let kind = match node.kind.as_str() {
            "Decision" => "decision",
            "DecisionRequest" => "decision-request",
            "Evidence" => "evidence",
            "Hypothesis" => "hypothesis",
            "Blocker" => "blocker",
            // Actor and Option have no standalone CaptureItem kind; skip.
            _ => continue,
        };
        let key = node.key.as_str();
        // Wire actor fields for kinds that support them within the current schema:
        // - Decision: accepted_by and rejected_by wired from gold edges (no gold case
        //   uses ProposedBy on a Decision node; actor_id stays unwired here).
        // - DecisionRequest: actor_id from DecisionRequestedBy/RequestProposedBy gold
        //   edges (whichever is present); accepted_by/rejected_by from
        //   RequestAcceptedBy/RequestRejectedBy gold edges. The projector derives which
        //   of DecisionRequestedBy/RequestProposedBy to emit from whether accepted_by/
        //   rejected_by are populated — both routes are gold-driven here, not guessed.
        // - BlockerForDecision and BlockerRequiredOwner need schema extensions;
        //   decision_id/blocked_actor_id left as None to avoid spurious edges.
        let (actor_id, accepted_by, rejected_by) = match kind {
            "decision" => (
                None,
                accepted_by_map.get(key).cloned().unwrap_or_default(),
                rejected_by_map.get(key).cloned().unwrap_or_default(),
            ),
            "decision-request" => (
                dr_actor_id_map.get(key).cloned(),
                accepted_by_map.get(key).cloned().unwrap_or_default(),
                rejected_by_map.get(key).cloned().unwrap_or_default(),
            ),
            _ => (None, Vec::new(), Vec::new()),
        };
        captures.push((
            key.to_owned(),
            CaptureItem {
                kind: kind.to_owned(),
                title: node.text.clone(),
                rationale: String::new(),
                topic_keys: vec![],
                evidence_ids: evidence_ids_map.get(key).cloned().unwrap_or_default(),
                options: options_map.get(key).cloned(),
                chosen_option: chosen_map.get(key).cloned(),
                extraction_confidence: 1.0,
                expressed_confidence: node.confidence.clone(),
                supersedes_id: supersedes_map.get(key).cloned(),
                premised_on_ids: premised_on_map.get(key).cloned().unwrap_or_default(),
                supports_ids: supports_map.get(key).cloned().unwrap_or_default(),
                refutes_ids: refutes_map.get(key).cloned().unwrap_or_default(),
                actor_id,
                accepted_by,
                rejected_by,
                blocked_actor_id: None,
                decision_id: None,
                participants: vec![],
                session_initiator: None,
            },
        ));
    }
    captures
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let corpus_path = parse_corpus_arg(&args);
    let ceiling_mode = args.iter().any(|a| a == "--ceiling");
    let rubric = parse_flag_value(&args, "--rubric")
        .map(|v| Rubric::parse(&v))
        .unwrap_or(Rubric::Exact);
    let show_mismatches = args.iter().any(|a| a == "--show-mismatches");
    let save_captures_path = parse_flag_value(&args, "--save-captures");
    let load_captures_path = parse_flag_value(&args, "--load-captures");
    let loaded_run: Option<CaptureRun> = load_captures_path.as_ref().map(|p| {
        let raw = std::fs::read_to_string(p).unwrap_or_else(|e| {
            eprintln!("error: cannot read --load-captures {p}: {e}");
            std::process::exit(1);
        });
        serde_json::from_str(&raw).unwrap_or_else(|e| {
            eprintln!("error: --load-captures {p} is not valid capture-cache JSON: {e}");
            std::process::exit(1);
        })
    });
    let loaded_captures: Option<HashMap<String, Vec<(String, hivemind::events::CaptureItem)>>> =
        loaded_run.as_ref().map(|run| {
            run.cases
                .iter()
                .cloned()
                .map(|c| (c.case_id, c.captures))
                .collect()
        });

    // (backend, model, api_key) — api_key only populated for the
    // anthropic-api backend; unused (and unset) in ceiling mode, replay mode
    // (--load-captures), or when the claude-cli backend is selected.
    let (backend, model, api_key): (Option<Backend>, String, String) = if ceiling_mode
        || loaded_captures.is_some()
    {
        (None, String::new(), String::new())
    } else {
        let backend = resolve_backend();
        match backend {
            Backend::AnthropicApi => match std::env::var("ANTHROPIC_API_KEY") {
                Ok(k) if !k.trim().is_empty() => {
                    (Some(backend), hivemind::classifier::resolved_model(), k)
                }
                _ => {
                    eprintln!(
                        "error: ANTHROPIC_API_KEY is not set; backend={} requires it",
                        backend.label()
                    );
                    eprintln!(
                        "hint: install/authenticate the `claude` CLI for the keyless claude-cli backend (set {EVAL_BACKEND_ENV}=claude-cli), or pass --ceiling"
                    );
                    std::process::exit(1);
                }
            },
            Backend::ClaudeCli => {
                if !claude_cli_on_path() {
                    eprintln!(
                        "error: {EVAL_BACKEND_ENV}=claude-cli but `claude` was not found on PATH (or failed to run)"
                    );
                    eprintln!(
                        "hint: install/authenticate the Claude Code CLI, set ANTHROPIC_API_KEY for the anthropic-api backend, or pass --ceiling"
                    );
                    std::process::exit(1);
                }
                (Some(backend), CLAUDE_CLI_MODEL.to_owned(), String::new())
            }
        }
    };

    // Provenance label used both in the printed scorecard and in the
    // capture-cache file (AGENTS.md §4: provenance is mandatory) — computed
    // once so ceiling/replay/live modes agree everywhere it's shown.
    let (run_backend_label, run_model_label): (String, String) = if ceiling_mode {
        ("ceiling".to_owned(), "n/a".to_owned())
    } else if let Some(run) = &loaded_run {
        (format!("replay:{}", run.backend), run.model.clone())
    } else {
        (
            backend
                .expect("resolved above when not in ceiling/replay mode")
                .label()
                .to_owned(),
            model.clone(),
        )
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

    if ceiling_mode {
        println!("HiveMind capture-fidelity evaluator — CEILING MODE");
        println!("(Gold nodes → CaptureItems via real projector; no LLM calls.)");
    } else if let Some(p) = &load_captures_path {
        println!("HiveMind capture-fidelity evaluator — REPLAY MODE");
        println!("(Captures loaded from {p}; no LLM calls.)");
    } else {
        println!("HiveMind capture-fidelity evaluator");
        println!(
            "Backend: {} (model: {model})",
            backend
                .expect("resolved above when not in ceiling/replay mode")
                .label()
        );
    }
    println!("Rubric: {}", rubric.label());
    println!("Corpus: {} cases", corpus.cases.len());
    if !corpus.org_bundles.is_empty() {
        println!(
            "Org bundles: {} (metadata for hmd-4tf, not scored)",
            corpus.org_bundles.len()
        );
    }
    println!();

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("reqwest client");

    let mut results: Vec<CaseResult> = Vec::new();
    let mut captures_out: Vec<CaseCaptures> = Vec::new();

    for case in &corpus.cases {
        println!("Running case {} ...", case.id);

        // id_captures: (stable_node_id, CaptureItem) pairs for the projector.
        let id_captures: Vec<(String, hivemind::events::CaptureItem)> = if ceiling_mode {
            gold_as_captures(&case.expected)
        } else if let Some(loaded) = &loaded_captures {
            loaded.get(&case.id).cloned().unwrap_or_else(|| {
                eprintln!(
                    "  warning: no cached captures for case {} in --load-captures file",
                    case.id
                );
                Vec::new()
            })
        } else {
            let classified = match backend.expect("resolved above when not in ceiling/replay mode")
            {
                Backend::AnthropicApi => {
                    hivemind::classifier::classify_text(&client, &api_key, &case.input).await
                }
                Backend::ClaudeCli => classify_text_cli(&case.input),
            };
            match classified {
                Ok(c) => c
                    .into_iter()
                    .enumerate()
                    .map(|(i, cap)| (format!("cap:{i}"), cap))
                    .collect(),
                Err(e) => {
                    eprintln!("  classifier error for {}: {e}", case.id);
                    // Score as empty produced — full recall penalty
                    Vec::new()
                }
            }
        };

        if save_captures_path.is_some() {
            captures_out.push(CaseCaptures {
                case_id: case.id.clone(),
                captures: id_captures.clone(),
            });
        }

        let id_capture_refs: Vec<(&str, &hivemind::events::CaptureItem)> = id_captures
            .iter()
            .map(|(id, cap)| (id.as_str(), cap))
            .collect();

        let (gold_nodes, gold_edges) = gold_graph(&case.expected);
        let (prod_nodes, prod_edges) = produced_graph(&id_capture_refs);

        if show_mismatches {
            print_mismatches(&case.id, &gold_nodes, &prod_nodes);
        }

        let node_counts = score_nodes_rubric(&gold_nodes, &prod_nodes, rubric);
        let edge_counts = score_edges_rubric(&gold_edges, &prod_edges, rubric);

        let result = CaseResult {
            case_id: case.id.clone(),
            node_counts,
            edge_counts,
            backend: run_backend_label.clone(),
            model: run_model_label.clone(),
        };

        print_case_scorecard(&result);
        results.push(result);
    }

    aggregate_scorecard(&results);

    if let Some(path) = &save_captures_path {
        let run = CaptureRun {
            backend: run_backend_label,
            model: run_model_label,
            cases: captures_out,
        };
        let json =
            serde_json::to_string_pretty(&run).expect("CaptureRun serialization cannot fail");
        std::fs::write(path, json).unwrap_or_else(|e| {
            eprintln!("error: cannot write --save-captures {path}: {e}");
            std::process::exit(1);
        });
        println!("\nSaved {} cases' captures to {path}", run.cases.len());
    }
}

/// Cached classifier output for a full corpus run — enough to re-score under
/// a different rubric without another LLM call (hivemind-g4ft.1: isolates
/// the rubric's effect on Macro-F1 from LLM run-to-run variance), with the
/// backend+model provenance of the original run preserved for replay.
#[derive(Debug, Serialize, Deserialize)]
struct CaptureRun {
    backend: String,
    model: String,
    cases: Vec<CaseCaptures>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CaseCaptures {
    case_id: String,
    captures: Vec<(String, hivemind::events::CaptureItem)>,
}

/// Returns the value following `flag` (e.g. `--rubric similarity` ->
/// `Some("similarity")`), or None if `flag` is absent.
fn parse_flag_value(args: &[String], flag: &str) -> Option<String> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == flag {
            return iter.next().cloned();
        }
    }
    None
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
    // Flags: --ceiling skips the LLM and uses gold nodes as produced output.
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
        let cap = hivemind::events::CaptureItem {
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
            premised_on_ids: vec![],
            supports_ids: vec![],
            refutes_ids: vec![],
            actor_id: None,
            accepted_by: vec![],
            rejected_by: vec![],
            blocked_actor_id: None,
            decision_id: None,
            participants: vec![],
            session_initiator: None,
        };
        let id_captures = [("d", &cap)];
        let (nodes, edges) = produced_graph(&id_captures);
        assert_eq!(nodes.len(), 3); // Decision + 2 Options
        assert!(nodes
            .iter()
            .any(|n| n.kind == "DECISION" || n.kind == "Decision"));
        assert!(nodes.iter().any(|n| n.text == "use postgres"));
        assert!(nodes.iter().any(|n| n.text == "postgres"));
        assert!(nodes.iter().any(|n| n.text == "sqlite"));
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

    // ----------------------------------------------------------------------
    // Phase 2 similarity rubric (hivemind-g4ft.1)
    // ----------------------------------------------------------------------

    #[test]
    fn text_similarity_identical_is_one() {
        assert!((text_similarity("use postgres", "use postgres") - 1.0).abs() < 1e-9);
        assert!((text_similarity("", "") - 1.0).abs() < 1e-9);
    }

    #[test]
    fn text_similarity_reworded_but_same_fact_clears_threshold() {
        // Same fact, LLM reworded it — exactly the case the bead hypothesizes
        // is tanking Macro-F1 under the Phase 1 exact rubric.
        let sim = text_similarity(
            "use postgres for the ledger store",
            "decided to use postgres as the ledger store",
        );
        assert!(
            sim >= SIMILARITY_THRESHOLD,
            "expected reworded match to clear threshold, got {sim}"
        );
    }

    #[test]
    fn text_similarity_unrelated_text_stays_below_threshold() {
        let sim = text_similarity(
            "use postgres for the ledger store",
            "hire a backend engineer",
        );
        assert!(
            sim < SIMILARITY_THRESHOLD,
            "expected unrelated text below threshold, got {sim}"
        );
    }

    #[test]
    fn text_similarity_empty_vs_nonempty_is_zero() {
        assert!((text_similarity("", "postgres") - 0.0).abs() < 1e-9);
        assert!((text_similarity("postgres", "") - 0.0).abs() < 1e-9);
    }

    #[test]
    fn score_nodes_rubric_exact_matches_score_nodes() {
        // Rubric::Exact must reproduce the legacy score_nodes exactly — no
        // behavior change for existing ceiling/CI scorecards.
        let gold = vec![
            ScoredNode {
                kind: "Decision".into(),
                text: "use postgres".into(),
            },
            ScoredNode {
                kind: "Evidence".into(),
                text: "load test".into(),
            },
        ];
        let produced = vec![ScoredNode {
            kind: "Decision".into(),
            text: "use postgres".into(),
        }];
        let legacy = score_nodes(&gold, &produced);
        let rubric = score_nodes_rubric(&gold, &produced, Rubric::Exact);
        for kind in ["Decision", "Evidence"] {
            let l = &legacy[kind];
            let r = &rubric[kind];
            assert_eq!((l.tp, l.fp, l.fn_), (r.tp, r.fp, r.fn_));
        }
    }

    #[test]
    fn score_nodes_rubric_similarity_recovers_reworded_match() {
        let gold = vec![ScoredNode {
            kind: "Decision".into(),
            text: "use postgres for the ledger store".into(),
        }];
        let produced = vec![ScoredNode {
            kind: "Decision".into(),
            text: "decided to use postgres as the ledger store".into(),
        }];

        let exact = score_nodes_rubric(&gold, &produced, Rubric::Exact);
        assert_eq!(
            exact["Decision"].tp, 0,
            "exact rubric should miss the reworded pair"
        );

        let similarity = score_nodes_rubric(&gold, &produced, Rubric::Similarity);
        assert_eq!(
            similarity["Decision"].tp, 1,
            "similarity rubric should recover the reworded pair"
        );
    }

    #[test]
    fn score_nodes_rubric_similarity_still_penalizes_fabrication() {
        // A produced node sharing no meaningful tokens with any gold node of
        // the same kind must still score as a false positive — the fuzzy
        // rubric must not become a rubber stamp.
        let gold: Vec<ScoredNode> = vec![];
        let produced = vec![ScoredNode {
            kind: "Decision".into(),
            text: "invented decision nobody made".into(),
        }];
        let counts = score_nodes_rubric(&gold, &produced, Rubric::Similarity);
        assert_eq!(counts["Decision"].fp, 1);
    }

    #[test]
    fn score_edges_rubric_similarity_requires_both_endpoints_close() {
        let gold = vec![ScoredEdge {
            kind: "HAS_OPTION".into(),
            from_kind: "Decision".into(),
            from_text: "use postgres for the ledger store".into(),
            to_kind: "Option".into(),
            to_text: "postgres".into(),
        }];
        // Right decision (reworded), wrong option entirely — must not match.
        let produced = vec![ScoredEdge {
            kind: "HAS_OPTION".into(),
            from_kind: "Decision".into(),
            from_text: "decision to use postgres as the ledger store".into(),
            to_kind: "Option".into(),
            to_text: "mysql".into(),
        }];
        let counts = score_edges_rubric(&gold, &produced, Rubric::Similarity);
        let c = &counts["HAS_OPTION"];
        assert_eq!((c.tp, c.fp, c.fn_), (0, 1, 1));
    }

    #[test]
    fn parse_flag_value_extracts_following_arg() {
        let args: Vec<String> = ["bin", "--rubric", "similarity", "--ceiling"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            parse_flag_value(&args, "--rubric"),
            Some("similarity".to_owned())
        );
        assert_eq!(parse_flag_value(&args, "--missing"), None);
    }
}
