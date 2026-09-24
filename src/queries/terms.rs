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

/// Question, function and decision-frame words that carry no identifying signal in a natural
/// question ("why did we decide to move the demo cell to shared Postgres", "what did we decide
/// about projects"). Deliberately omits negations (`not`, `no`, `never`, `without`): dropping
/// them would let "do not adopt Kafka" resolve to the decision that adopted it.
const QUESTION_STOPWORDS: &[&str] = &[
    "a",
    "about",
    "an",
    "and",
    "are",
    "as",
    "at",
    "be",
    "been",
    "but",
    "by",
    "can",
    "chose",
    "choose",
    "chosen",
    "could",
    "decide",
    "decided",
    "decision",
    "decisions",
    "did",
    "do",
    "does",
    "for",
    "from",
    "had",
    "has",
    "have",
    "how",
    "i",
    "if",
    "in",
    "into",
    "is",
    "it",
    "its",
    "me",
    "my",
    "of",
    "on",
    "or",
    "our",
    "should",
    "so",
    "than",
    "that",
    "the",
    "their",
    "them",
    "then",
    "these",
    "they",
    "this",
    "those",
    "to",
    "us",
    "was",
    "we",
    "were",
    "what",
    "when",
    "where",
    "which",
    "who",
    "whom",
    "why",
    "will",
    "with",
    "would",
    "you",
    "your",
];

/// Lowercased whitespace tokens with surrounding punctuation trimmed, duplicates removed. Inner
/// punctuation is kept (`per-host`, `gc-ox429`); a token that is all punctuation keeps its raw
/// form so it still matches literally.
fn description_tokens(description: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    for raw in description.split_whitespace() {
        let token = raw
            .trim_matches(|c: char| !c.is_alphanumeric())
            .to_ascii_lowercase();
        let token = if token.is_empty() {
            raw.to_ascii_lowercase()
        } else {
            token
        };
        if !tokens.contains(&token) {
            tokens.push(token);
        }
    }
    tokens
}

fn without_stopwords(tokens: &[String]) -> Vec<String> {
    tokens
        .iter()
        .filter(|token| !QUESTION_STOPWORDS.contains(&token.as_str()))
        .cloned()
        .collect()
}

/// Terms for resolving a free-text description to a decision: the description's tokens minus
/// question words. Falls back to the unfiltered tokens when every token is a stopword, so
/// "why did we" never matches every decision.
pub(crate) fn resolver_terms(description: &str) -> Vec<String> {
    let tokens = description_tokens(description);
    let content = without_stopwords(&tokens);
    if content.is_empty() {
        tokens
    } else {
        content
    }
}

/// A free-text query split into what to search for and what was left out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentQuery {
    /// The query with its question words removed ("what did we decide about projects" ->
    /// "projects"), or `None` when nothing else is left ("what did we decide"): a bare question
    /// adds no filter, so the caller's other filters (topic, status, ...) decide the result.
    pub query: Option<String>,
    /// The question words that were dropped, in the order they were asked.
    pub ignored: Vec<String>,
}

pub fn content_query(text: &str) -> ContentQuery {
    let tokens = description_tokens(text);
    let content = without_stopwords(&tokens);
    let ignored = tokens
        .into_iter()
        .filter(|token| !content.contains(token))
        .collect();
    ContentQuery {
        query: if content.is_empty() {
            None
        } else {
            Some(content.join(" "))
        },
        ignored,
    }
}

/// Reduce an English word to a stem so inflections compare equal: `move`, `moves`, `moved` and
/// `moving` all become `mov`; `policy` and `policies` become `polic`. Plural, then `-ed`/`-ing`,
/// then a trailing `e`/`y` are stripped, each only when at least three letters remain. Words with
/// non-letters (ids, `per-host`, `3f2a`) are returned unchanged. Compared for equality, never as
/// a substring, so a short stem cannot match unrelated words.
pub(crate) fn stem(word: &str) -> &str {
    if !word.bytes().all(|b| b.is_ascii_alphabetic()) {
        return word;
    }
    let mut root = word;
    if let Some(stripped) = root.strip_suffix("ies").filter(|s| s.len() >= 3) {
        root = stripped;
    } else if let Some(stripped) = root.strip_suffix("es").filter(|s| s.len() >= 3) {
        root = stripped;
    } else if let Some(stripped) = root
        .strip_suffix('s')
        .filter(|s| s.len() >= 3 && !s.ends_with(['s', 'u', 'i']))
    {
        root = stripped;
    }
    if let Some(stripped) = root
        .strip_suffix("ing")
        .or_else(|| root.strip_suffix("ed"))
        .filter(|s| s.len() >= 3)
    {
        root = stripped;
    }
    if let Some(stripped) = root.strip_suffix(['e', 'y']).filter(|s| s.len() >= 3) {
        root = stripped;
    }
    root
}

/// The stem of every word in `text` (split on non-alphanumerics), for matching a stemmed term
/// against a field's words. `text` must already be lowercase.
pub(crate) fn word_stems(text: &str) -> BTreeSet<&str> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(stem)
        .collect()
}
