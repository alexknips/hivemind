//! Deterministic term extraction and overlap scoring shared across resolve-by-description
//! (hivemind-tenv.1) and situational path/evidence matching (hivemind-tenv.2). Plain string
//! splitting and set intersection only — no LLM, no learned weights, no fuzzy/edit-distance
//! matching (AGENTS.md Principles 1/7). Whichever bead's implementation lands first keeps
//! this module; the other adopts it rather than forking a second ranker (see hivemind-tenv).

use std::collections::{BTreeSet, HashMap};
use std::sync::LazyLock;

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

/// Where a negation in a title stops reaching: a comma, semicolon, colon, bracket or dash ends a
/// clause.
const CLAUSE_BREAKS: &[char] = &[',', ';', ':', '(', ')', '\u{2013}', '\u{2014}'];

/// The words of `text` (a decision's title) that no negation reaches, joined by spaces. A negation
/// reaches the words after it to the end of its clause, so "Decision links use title slugs, not
/// slugs remembered per browser" says "decision links use title slugs" outright and denies only
/// "slugs remembered per browser", while "Do not adopt Kafka" says nothing outright about Kafka.
/// Whole words only, so "Notion" and "note" are not "not".
pub(crate) fn outside_negation(text: &str) -> String {
    let mut outright: Vec<&str> = Vec::new();
    for clause in text.split(CLAUSE_BREAKS) {
        outright.extend(
            clause
                .split(|c: char| !c.is_alphanumeric() && c != '\'' && c != '\u{2019}')
                .map(|word| word.trim_matches(['\'', '\u{2019}']))
                .filter(|word| !word.is_empty())
                .take_while(|word| !is_negation(&word.to_lowercase().replace('\u{2019}', "'"))),
        );
    }
    outright.join(" ")
}

/// Lowercased whitespace tokens in the order asked, with surrounding punctuation trimmed and a
/// typographic apostrophe (`don’t`) written straight. Inner punctuation is kept (`per-host`,
/// `gc-ox429`); a token that is all punctuation keeps its raw form so it still matches literally.
/// A possessive is the word it belongs to (`website's` is `website`): a decision's text says
/// "the website", not "the website's".
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
                match token.strip_suffix("'s") {
                    Some(owner) if !owner.is_empty() => owner.to_owned(),
                    _ => token,
                }
            }
        })
        .collect()
}

/// The forms of "make" people ask about a decision with: "which agent made a decision", "why did
/// we make the decision to ...".
const MAKING_WORDS: &[&str] = &["make", "makes", "made", "making"];

/// Words that may stand between "made" and the decision it made.
const DETERMINERS: &[&str] = &["a", "an", "the", "this", "that", "any", "some", "each"];

/// Whether the first of `tokens` is a form of "make" in front of the decision it makes ("made a
/// decision", "make decisions"): question framing, like "pick" or "decide", and only there. Alone it
/// is a real word ("what makes a link the same", "made readable").
fn is_decision_making(tokens: &[String]) -> bool {
    let [verb, rest @ ..] = tokens else {
        return false;
    };
    if !MAKING_WORDS.contains(&verb.as_str()) {
        return false;
    }
    let is_decision = |word: &String| matches!(word.as_str(), "decision" | "decisions");
    match rest {
        [next, ..] if is_decision(next) => true,
        [next, after, ..] => DETERMINERS.contains(&next.as_str()) && is_decision(after),
        _ => false,
    }
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
    let tokens = description_tokens(description);
    let making: Vec<bool> = (0..tokens.len())
        .map(|index| tokens.get(index..).is_some_and(is_decision_making))
        .collect();
    let mut tokens = tokens.into_iter().zip(making).peekable();
    while let Some((token, making)) = tokens.next() {
        let negation = is_negation(&token);
        split.negated |= negation;
        let framing = negation
            || making
            || is_question_word(&token, tokens.peek().map(|(next, _)| next.as_str()));
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
    /// A decision whose own title says every one of `terms` outright is the opposite of what was
    /// asked and never answers it: "don't adopt Kafka" must never resolve to the decision "Adopt
    /// Kafka". A title that leaves a term out, or denies it, is not the opposite of anything. A
    /// negation is never one of `terms`.
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
    /// match equally, one whose own title does not say every term outright comes first.
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
    if !is_plain_word(word) {
        return word;
    }
    strip_inflection(strip_plural(word))
}

/// Letters only: the words the suffix rules below apply to. An id, `per-host` or `3f2a` is
/// compared as written.
fn is_plain_word(word: &str) -> bool {
    word.bytes().all(|b| b.is_ascii_alphabetic())
}

/// `-ies`, `-es` or `-s` off a lowercase word, each only when at least three letters remain.
fn strip_plural(word: &str) -> &str {
    if let Some(stripped) = word.strip_suffix("ies").filter(|s| s.len() >= 3) {
        stripped
    } else if let Some(stripped) = word.strip_suffix("es").filter(|s| s.len() >= 3) {
        stripped
    } else if let Some(stripped) = word
        .strip_suffix('s')
        .filter(|s| s.len() >= 3 && !s.ends_with(['s', 'u', 'i']))
    {
        stripped
    } else {
        word
    }
}

/// `-ing` or `-ed`, then a trailing `e` or `y`, each only when at least three letters remain.
fn strip_inflection(root: &str) -> &str {
    let root = root
        .strip_suffix("ing")
        .or_else(|| root.strip_suffix("ed"))
        .filter(|s| s.len() >= 3)
        .unwrap_or(root);
    strip_final_vowel(root)
}

fn strip_final_vowel(root: &str) -> &str {
    root.strip_suffix(['e', 'y'])
        .filter(|s| s.len() >= 3)
        .unwrap_or(root)
}

/// The stem of every word in `text` (split on non-alphanumerics), for matching a stemmed term
/// against a field's words. `text` must already be lowercase.
pub(crate) fn word_stems(text: &str) -> BTreeSet<&str> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(stem)
        .collect()
}

/// Suffixes that make a noun or an adjective of a verb or a noun (`accept` -> `acceptance`,
/// `refute` -> `refutation`, `move` -> `movement`) or an adverb of an adjective (`public` ->
/// `publicly`), each with the fewest letters that must be left once it is off, so a short word is
/// never cut to a stub: `section` is not `sect`, `former` is not `form`, `only` is not `on`.
const DERIVED_SUFFIXES: &[(&str, usize)] = &[
    ("ation", 4),
    ("ition", 4),
    ("ion", 5),
    ("ment", 4),
    ("ance", 4),
    ("ence", 4),
    ("ness", 4),
    ("ity", 5),
    ("al", 5),
    ("er", 5),
    ("or", 5),
    ("ly", 5),
];

/// The keys under which `recall` compares a lowercase `word`: two words are forms of one word when
/// they share a key.
///
/// - its stem (`stem`);
/// - the word with its plural off and nothing else, for a form the stem cuts twice (`supersedes`
///   is `supersed` here and `super` as a stem);
/// - what a noun or adverb suffix was added to (`acceptance` -> `accept`, `movement` -> `mov`,
///   `publicly` -> `public`);
/// - the part a verb in `-d`/`-de` and its noun in `-sion` share (`supersede` and `supersession`
///   share `superse`, `decide` and `decision` share `deci`, `expand` and `expansion` share
///   `expan`).
///
/// Every key is a leading part of the word, so a text holds a form of a word only if it holds one
/// of its keys. A word with anything but letters in it (an id, `per-host`) has no key but itself.
fn word_keys(word: &str) -> Vec<&str> {
    if !is_plain_word(word) {
        return vec![word];
    }
    let plain = strip_plural(word);
    let mut keys = vec![stem(word), strip_final_vowel(plain)];
    for (suffix, kept) in DERIVED_SUFFIXES {
        if let Some(base) = plain
            .strip_suffix(suffix)
            .filter(|base| base.len() >= *kept)
        {
            keys.push(strip_final_vowel(base));
        }
    }
    let verbs: Vec<&str> = keys
        .iter()
        .copied()
        .filter(|key| key.len() >= 5)
        .filter_map(|key| key.strip_suffix('d'))
        .collect();
    keys.extend(verbs);
    if let Some(root) = plain
        .strip_suffix("ssion")
        .or_else(|| plain.strip_suffix("sion"))
        .filter(|root| root.len() >= 4)
    {
        keys.push(root);
    }
    keys
}

/// Groups of words that say the same thing in this product's own vocabulary, so a question that
/// uses one finds a decision that uses another: the site draws a supersession as "replaces", the
/// UI says "assumption" where the code says "hypothesis", one asker's "picture" is another's
/// "graph", and what the app calls an edge the site's diagrams draw as an arrow. Fixed and
/// literal, like `QUESTION_STOPWORDS`: nothing is learned and nothing is guessed. Any word of a
/// group, and any form of it, stands in for any other. A stand-in is never the word itself: a
/// decision that has the word asked for outranks one that only has a stand-in (`WordMatch`). The
/// groups are disjoint, and kept small on purpose: a word belongs here only when people do ask
/// about the same decision with either of them.
pub(crate) const WORD_GROUPS: &[&[&str]] = &[
    &["supersede", "supersession", "replace"],
    &[
        "refute",
        "disprove",
        "disproven",
        "falsify",
        "invalidate",
        "wrong",
    ],
    &["assumption", "hypothesis", "hypotheses", "premise"],
    &["ui", "interface"],
    &["graph", "diagram", "chart", "picture"],
    &["site", "website"],
    &["edge", "arrow"],
    &["link", "url", "address"],
    &["browser", "device", "laptop", "phone", "mobile", "desktop"],
];

struct Vocabulary {
    /// The group each key of a listed word belongs to.
    group_of: HashMap<&'static str, usize>,
    /// The keys of every word of each group: what a text must contain for a stand-in to be in it.
    group_keys: Vec<Vec<&'static str>>,
}

static VOCABULARY: LazyLock<Vocabulary> = LazyLock::new(|| {
    let mut group_of = HashMap::new();
    let mut group_keys = Vec::new();
    for (group, words) in WORD_GROUPS.iter().enumerate() {
        let keys: BTreeSet<&'static str> = words.iter().copied().flat_map(word_keys).collect();
        for key in &keys {
            group_of.entry(*key).or_insert(group);
        }
        group_keys.push(keys.into_iter().collect());
    }
    Vocabulary {
        group_of,
        group_keys,
    }
});

/// How a text holds a word of the question.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WordMatch {
    /// The word itself, or a form of it (`word_keys`).
    Word,
    /// A word `WORD_GROUPS` lists as standing in for it.
    StandIn,
}

/// One word of a question, ready to be looked for in the text of a decision.
pub(crate) struct RelatedWord<'a> {
    keys: BTreeSet<&'a str>,
    group: Option<usize>,
}

impl<'a> RelatedWord<'a> {
    /// `word` must be lowercase.
    pub(crate) fn new(word: &'a str) -> Self {
        let keys: BTreeSet<&str> = word_keys(word).into_iter().collect();
        let group = keys
            .iter()
            .find_map(|key| VOCABULARY.group_of.get(*key).copied());
        Self { keys, group }
    }

    /// How `text` (lowercase) holds this word: as itself or a form of it, else as a stand-in, else
    /// not at all. Whole words are compared, never prefixes, so `string` is not in `strategy`.
    pub(crate) fn find_in(&self, text: &str) -> Option<WordMatch> {
        // Every key is a leading part of its word, so a text holding none of them holds no form
        // of the word and no stand-in: the common case costs one substring scan per key.
        let stand_in_keys: &[&str] = self
            .group
            .and_then(|group| VOCABULARY.group_keys.get(group))
            .map_or(&[], Vec::as_slice);
        if !self
            .keys
            .iter()
            .chain(stand_in_keys)
            .any(|key| text.contains(key))
        {
            return None;
        }
        let mut found = None;
        for word in text
            .split(|c: char| !c.is_alphanumeric())
            .filter(|word| !word.is_empty())
        {
            for key in word_keys(word) {
                if self.keys.contains(key) {
                    return Some(WordMatch::Word);
                }
                if self.group.is_some() && VOCABULARY.group_of.get(key).copied() == self.group {
                    found = Some(WordMatch::StandIn);
                }
            }
        }
        found
    }
}
