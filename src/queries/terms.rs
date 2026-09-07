//! Deterministic term extraction and overlap scoring shared across resolve-by-description
//! (hivemind-tenv.1) and situational path/evidence matching (hivemind-tenv.2). Plain string
//! splitting and set intersection only — no LLM, no learned weights, no fuzzy/edit-distance
//! matching (AGENTS.md Principles 1/7). Whichever bead's implementation lands first keeps
//! this module; the other adopts it rather than forking a second ranker (see hivemind-tenv).

use std::collections::BTreeSet;

/// Segment separators inside a file path or branch-like string.
const PATH_SEPARATORS: &[char] = &['/', '\\', '_', '-', '.', ' '];

/// Generic tokens that carry no situational signal: common source-tree directory names,
/// file extensions, and short prose stopwords. Dropped so extracted terms don't drown
/// real signal in "src"/"rs"/"the" noise. This is a fixed, deterministic list — not a
/// learned or configurable one.
const STOPWORDS: &[&str] = &[
    "src", "lib", "mod", "bin", "test", "tests", "spec", "specs", "the", "and", "for", "with",
    "of", "in", "on", "at", "to", "a", "an", "is", "it", "rs", "ts", "tsx", "js", "jsx", "py",
    "go", "java", "rb", "c", "cpp", "h", "hpp", "md", "json", "toml", "yml", "yaml", "html", "css",
    "txt", "lock", "sh",
];

/// Extract deterministic, lowercased terms from a file path or branch-like string.
/// Splits on path separators, underscores, hyphens, dots, and whitespace; drops
/// empty/single-character segments and common stopwords/extensions.
pub fn path_terms(path: &str) -> Vec<String> {
    normalized_terms(path)
}

/// Extract deterministic, lowercased terms from free-form prose (evidence content,
/// rationale). Same splitting rules as `path_terms` — this is a term-overlap
/// heuristic over free text, not a structured index (evidence has no `paths` field).
pub fn text_terms(text: &str) -> Vec<String> {
    normalized_terms(text)
}

fn normalized_terms(input: &str) -> Vec<String> {
    input
        .split(|c: char| PATH_SEPARATORS.contains(&c) || (!c.is_alphanumeric() && c != '\''))
        .map(|segment| segment.to_ascii_lowercase())
        .filter(|segment| segment.len() > 1 && !STOPWORDS.contains(&segment.as_str()))
        .collect()
}

/// Terms from `query_terms` that also appear in `candidate_terms`, sorted and deduped.
pub fn overlapping_terms(query_terms: &[String], candidate_terms: &[String]) -> Vec<String> {
    let candidates: BTreeSet<&str> = candidate_terms.iter().map(String::as_str).collect();
    let mut hits: Vec<String> = query_terms
        .iter()
        .filter(|term| candidates.contains(term.as_str()))
        .cloned()
        .collect();
    hits.sort();
    hits.dedup();
    hits
}

/// Fraction of the unique terms in `query_terms` found in `candidate_terms`. `0.0` when
/// `query_terms` is empty or there is no overlap; `1.0` when every query term is present
/// in the candidate set. Deterministic term-frequency overlap, not BM25 or any
/// learned/probabilistic scoring (AGENTS.md Principles 1/7).
pub fn overlap_score(query_terms: &[String], candidate_terms: &[String]) -> f64 {
    let unique: BTreeSet<&str> = query_terms.iter().map(String::as_str).collect();
    if unique.is_empty() {
        return 0.0;
    }
    let candidates: BTreeSet<&str> = candidate_terms.iter().map(String::as_str).collect();
    let hits = unique.intersection(&candidates).count();
    hits as f64 / unique.len() as f64
}
