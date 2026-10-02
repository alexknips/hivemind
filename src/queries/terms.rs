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
/// question ("why did we decide to move the demo cell to shared Postgres", "why did we pick
/// shadcn", "why is the demo still on the site"): the verbs people use to ask about a decision
/// (pick, choose, decide) and the adverbs they put in a why-question (still, again, ever, ...).
/// Fixed and literal: `-s` and past forms are listed, nothing is stemmed, no synonyms. They are
/// dropped from the question only, never from a decision's text, so a decision titled "Pick the
/// cheapest vendor" still matches on "pick".
///
/// Negations (`not`, `doesn't`, ...) are not here: they are not framing but polarity, and have one
/// rule of their own (see `is_negation`).
const QUESTION_STOPWORDS: &[&str] = &[
    "a",
    "about",
    "actually",
    "again",
    "an",
    "and",
    "anymore",
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
    "chooses",
    "chosen",
    "could",
    "currently",
    "decide",
    "decided",
    "decides",
    "decision",
    "decisions",
    "did",
    "do",
    "does",
    "even",
    "ever",
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
    "now",
    "of",
    "on",
    "or",
    "our",
    "pick",
    "picked",
    "picks",
    "really",
    "should",
    "so",
    "still",
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

/// Two-word decision verbs ("go with", "settle on", "opt for"). The first word is question framing
/// only in front of its partner: alone it is a real term ("go" is a language, "opt" a directory),
/// so "why did we go with Go" still searches for `go`. The partners (`with`, `on`, `for`) are
/// question words already.
const FRAMING_PHRASES: &[(&str, &str)] = &[
    ("go", "with"),
    ("goes", "with"),
    ("went", "with"),
    ("opt", "for"),
    ("opted", "for"),
    ("opts", "for"),
    ("settle", "on"),
    ("settled", "on"),
    ("settles", "on"),
];

/// Words that negate on their own. A contraction of a negated auxiliary (`doesn't`, `won't`,
/// `can't`, ...) ends in `n't`; `is_negation` covers both.
const NEGATION_WORDS: &[&str] = &["cannot", "never", "no", "not", "without"];

/// Whether `word` (lowercase, apostrophes straight) is a negation, bare (`not`, `no`, `never`,
/// `without`, `cannot`) or contracted (`doesn't`, `don't`, `isn't`, ...). One rule for every
/// spelling, so "do not adopt Kafka" and "don't adopt Kafka" are asked the same way.
fn is_negation(word: &str) -> bool {
    NEGATION_WORDS.contains(&word) || word.ends_with("n't")
}

/// Whether `text` (a decision's title) says something negated: any of its words is a negation.
/// Whole words only, so "Notion" and "note" are not "not".
pub(crate) fn is_negated_text(text: &str) -> bool {
    text.split(|c: char| !c.is_alphanumeric() && c != '\'' && c != '\u{2019}')
        .map(|word| word.trim_matches(['\'', '\u{2019}']))
        .any(|word| is_negation(&word.to_lowercase().replace('\u{2019}', "'")))
}

/// Lowercased whitespace tokens in the order asked, with surrounding punctuation trimmed and a
/// typographic apostrophe (`don’t`) written straight. Inner punctuation is kept (`per-host`,
/// `gc-ox429`); a token that is all punctuation keeps its raw form so it still matches literally.
fn description_tokens(description: &str) -> Vec<String> {
    description
        .split_whitespace()
        .map(|raw| {
            let token = raw
                .trim_matches(|c: char| !c.is_alphanumeric())
                .to_ascii_lowercase()
                .replace('\u{2019}', "'");
            if token.is_empty() {
                raw.to_ascii_lowercase()
            } else {
                token
            }
        })
        .collect()
}

/// Whether `token`, followed by `next`, frames the question rather than naming what it asks about.
fn is_question_word(token: &str, next: Option<&str>) -> bool {
    QUESTION_STOPWORDS.contains(&token)
        || FRAMING_PHRASES
            .iter()
            .any(|(lead, partner)| *lead == token && next == Some(*partner))
}

/// A description split into what to search for and the words around it. Each word appears once,
/// in the order asked.
struct QuestionTokens {
    content: Vec<String>,
    /// Question words and negations: what is asked with, not what is asked about.
    framing: Vec<String>,
    /// Whether the question says "not" in any form. A negation is polarity, never a term to find.
    negated: bool,
}

fn question_tokens(description: &str) -> QuestionTokens {
    let mut split = QuestionTokens {
        content: Vec::new(),
        framing: Vec::new(),
        negated: false,
    };
    let mut tokens = description_tokens(description).into_iter().peekable();
    while let Some(token) = tokens.next() {
        let negation = is_negation(&token);
        split.negated |= negation;
        let framing = negation || is_question_word(&token, tokens.peek().map(String::as_str));
        let bucket = if framing {
            &mut split.framing
        } else {
            &mut split.content
        };
        if !bucket.contains(&token) {
            bucket.push(token);
        }
    }
    split
}

/// What a free-text description asks a decision to be: the terms it must contain and whether it
/// asks for a negated one.
pub(crate) struct ResolverQuestion {
    pub(crate) terms: Vec<String>,
    /// The description says "not" in some form (`not`, `no`, `never`, `without`, `doesn't`, ...).
    /// Only a decision whose own title is negated answers it: "don't adopt Kafka" must never
    /// resolve to the decision "Adopt Kafka". A negation is never one of `terms`.
    pub(crate) negated: bool,
}

/// Terms for resolving a free-text description to a decision: the description's tokens minus
/// question words and negations. Falls back to the unfiltered tokens when nothing else is left,
/// so "why did we" never matches every decision; those tokens are then searched as written and
/// the question is not read as negated.
pub(crate) fn resolver_question(description: &str) -> ResolverQuestion {
    let QuestionTokens {
        content,
        framing,
        negated,
    } = question_tokens(description);
    // No content means every token is framing, so `framing` holds them all.
    if content.is_empty() {
        ResolverQuestion {
            terms: framing,
            negated: false,
        }
    } else {
        ResolverQuestion {
            terms: content,
            negated,
        }
    }
}

/// The terms of `resolver_question`.
pub(crate) fn resolver_terms(description: &str) -> Vec<String> {
    resolver_question(description).terms
}

/// A free-text query split into what to search for and what was left out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentQuery {
    /// The query with its question words removed ("what did we decide about projects" ->
    /// "projects"), or `None` when nothing else is left ("what did we decide"): a bare question
    /// adds no filter, so the caller's other filters (topic, status, ...) decide the result.
    pub query: Option<String>,
    /// The question words and negations that were dropped, in the order they were asked.
    pub ignored: Vec<String>,
    /// The question says "not" in some form. It does not narrow the answer: among decisions that
    /// match equally, one whose own title is negated comes first.
    pub negated: bool,
}

pub fn content_query(text: &str) -> ContentQuery {
    let QuestionTokens {
        content,
        framing,
        negated,
    } = question_tokens(text);
    // A word that frames the question in one place and is asked about in another ("go with Go")
    // is searched for, so it is not reported as dropped.
    let ignored = framing
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
        negated: negated && !content.is_empty(),
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
