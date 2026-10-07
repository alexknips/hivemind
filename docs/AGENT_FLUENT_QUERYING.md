# Agent-Fluent Follow-Up Verbs — Design Document (tenv.1)

Status: **SHIPPED** (was DRAFT as of 2026-09-07). The CLI surface landed via
`hivemind-tenv.1`/`hivemind-tenv.2`. MCP+HTTP backend parity for the resolver
and the `why`/`verify`/`disagree`/`supersede` verbs landed via
`hivemind-ot72.5`–`hivemind-ot72.8` (2026-09-21); `get_supersession_chain` and
`compact-view` remain `decision_id`-only over MCP (`hivemind-ot72.9`,
`hivemind-ot72.10`, both still open). §1.5 below covers backend portability
and §3.4 covers current MCP status. The §7 open questions below are the
historical record of what alex actually decided at each checkpoint — kept for
the rationale, not because the design is still speculative. See also
[`docs/AGENT_DECISION_CONTEXT.md`](AGENT_DECISION_CONTEXT.md) for how this
resolver fits into the two shipped interaction models (MCP vs. the
`hivemind-context` CLI plugin).
Bead: hivemind-tenv.1
Parent epic: hivemind-tenv
Author: gastown.rictus (polecat), 2026-09-07

---

## 0. Purpose and Scope

This is a **plan-first deliverable**: no product code beyond the exploratory
reading already done for this document is written until alex reviews and
approves the design (see docs/INGESTION_CONNECTORS.md for the precedent this
document follows).

Today the only free-text entry points into HiveMind's query layer are
`recall <query>` (`src/cli/args.rs:1007`, positional `Option<String>`),
`search --q <query>` (`src/cli/args.rs:986`), and
`get_relevant_decisions --topic <topic>` (`src/cli/args.rs:950`). Every
follow-up verb — `disagree`, `supersede`, `get_supersession_chain`,
`get_decision_neighborhood`, `compact-view` — requires `--id`
(`src/cli/args.rs:281,290,944,959,912`), and every equivalent MCP tool
requires `decision_id` as a required string argument (`src/mcp.rs:395`,
`:441`, `:464`, `:546`). An agent that just ran `recall` knows a decision's
*content*, not an opaque id, unless it round-trips the id back out of the
JSON response first. This document designs the primitive that closes that
gap and the contract every follow-up verb adopts.

Scope (this bead, tenv.1):
- One shared resolve-by-description primitive in `src/queries`.
- Free-text positional input (with `--id` / `--pick N` escape hatches) on:
  `disagree`, `supersede`, `get_supersession_chain`, `get_decision_neighborhood`
  (aliased `why`), `compact-view`, and a **new** `review` query verb backed by
  `get_decision_outcome` (see §3.1 — this is not the existing `review`
  subcommand).
- A shared output contract (`DecisionBrief`) used by every verb above.
- The short-handle continuation mechanism (`#N`, `--from-last`).
- Tests on both backends (SQLite `MemoryGraph`/`SqliteEventLedger` +
  `shared-backend-postgres` feature).

Out of scope (owned by siblings per the parent epic):
- `hivemind query situational` (working name at the time this document was
  written: `hivemind query context`; naming locked 2026-09-08 to
  `situational` — see hivemind-tenv's bead notes) — reuses this bead's
  resolver module if it lands first; coordinate through bead comments per
  tenv.2's description, do not fork a second ranker.
- The `hivemind-context` Claude/Codex plugin (tenv.3).
- Repointing `hivemind-capture`'s query script and docs/website (tenv.4).
- MCP tool schema changes are **sketched** in §3.4 for parity but the MCP
  server wiring itself is implementation work under this same bead once the
  design is approved — flagged, not deferred to another bead.

**Relationship to existing search design docs:** `docs/SEARCH_DESIGN.md`
already states the mission this bead completes — "the caller can find a
decision without already knowing its id, then decide whether to inspect,
cite, contest, or **supersede** it" (`SEARCH_DESIGN.md:16`) and "Offset
bounds are canonical. Timestamp bounds are resolved to concrete event
offset bounds" (`SEARCH_DESIGN.md:184`) — but it only specifies the *find*
step; contest/supersede-by-description was never designed, and that offset-
canonical doctrine directly supports treating `event_origin` as the
canonical recency signal (§1.2) rather than treating the missing wall-clock
timestamp as a blocking gap. `docs/DECISION_SEARCH_QUERY.md` documents the
same rank-tier system this design reuses (§1.1 below).

---

## 1. The Resolver Primitive

### 1.1 Why generalize rather than invent

`src/queries/search.rs` already contains a deterministic, backend-agnostic
matcher: `collect_graph_search_results` (`search.rs:569`) builds
`ScoredDecisionSearchResult`s by calling `evaluate_search_match`
(`search.rs:838`), which computes a `rank: u8` tier per decision — `0` for an
exact id or title match, or the exact question the decision answers
(`exact_question_match`; case and a closing `?` do not count),
otherwise `min` over matched-field ranks, requiring **all** query terms
(`shared::query_terms`, `shared.rs:30` — lowercased whitespace split) to
match somewhere (AND semantics). This already runs against `impl GraphView`,
so it works unmodified against both `MemoryGraph`/SQLite-projected graphs and
`PostgresGraphView` (proven by `src/projector/postgres/tests.rs:14`, which
imports `search_decisions` directly and asserts parity against `MemoryGraph`
via `with_postgres_graph`, `tests.rs:227`).

**Decision:** the resolver is not a new ranking algorithm. It is
`collect_graph_search_results`'s tier system, extracted into a shared
function and extended with a recency tiebreak, wrapped in an ambiguity gate.
No new matching logic, no configuration knobs, no learned weights.

**The question a decision answers is searched too (hivemind-pyy4).** A decision
records the question it answers (`--question`, `capture --answers`, `ground --answers`,
`request_decision`, every AskUserQuestion answer the ask hooks write), and that text is a
field of the decision beside its rationale: `why`, `verify`, `recall` and `search` all read
it, so asking a decision's own question back finds it. For the decisions the ask hooks write,
titled "<header>: <choice>" with a fixed rationale, the question is the only text a person
would ask with. It matches as the rationale does (tier 2, `decision.question` in
`matched_fields`), and a description that is the recorded question itself is an exact match
(tier 0, as an exact title is). A question that several decisions answer is therefore
ambiguous between them, never resolved to one: the answers are the candidates. A question
quoted as recorded also carries its own polarity: "... or C no Jev?" or "... without judging"
does not make the decision it answers the opposite of what was asked (the negation rule below
is for a description that states the opposite of a decision's title).

**Term handling (resolver only).** A natural question resolves the same as
its keywords: "why did we move the demo cell to shared Postgres" finds
"Demo cell storage moves to shared Postgres backend…".
`resolver_terms` (`src/queries/terms.rs`) trims punctuation and drops question
and function words (why, did, we, the, to, …) and negations (one rule, see
**Negation** below). The verbs people ask about a decision with (pick, choose, decide,
and "go with", "settle on", "opt for", each with its past and `-s` forms) and
the adverbs they put in a why-question (still, again, ever, even, really,
actually, now, anymore, currently) are question words too, and so is "make" in front of
the decision it makes ("which agent made a decision", "make the decision to"; alone it stays
a word: "what makes a link the same"), so "why did we pick
shadcn for the design system" resolves in one step instead of listing the
decision as a close candidate missing "pick". The list is fixed and literal —
no stemming, no synonyms — and applies to the question only: a decision titled
"Pick the cheapest vendor" still matches on "pick". A two-word verb is dropped
only as a pair ("go" alone stays a term: it is a language). Each remaining term
still has to match somewhere (AND), either as a substring, as before, or as a
field word with the same stem (`move`/`moves`/`moved`/`moving`; compared by
whole-word equality, never by prefix, so `string` does not match `strategy`),
and a repeated word counts once. A description made only of stop words falls back to its literal words.
The ambiguity gate (§1.4) is unchanged, and `search` keeps literal substring
matching.

**Negation.** One rule for every spelling of "not" (hivemind-m974, hivemind-g889):
the bare `not`, `no`, `never`, `without` and `cannot`, and any contraction that
ends in `n't` (doesn't, don't, isn't, aren't, won't, didn't, can't, …), with a
typographic apostrophe read as a straight one. A negation is polarity, not a
word to find.

- It is never a term. It is never listed in `missing_terms` and never lowers a
  candidate's rank: a decision's text almost never contains "doesn't" verbatim,
  and a "not" matched as a word counted against every candidate, so the right
  decision sank behind any decision that shared one more ordinary word.
- A negated question never resolves, for `why`, `verify`, `chain`, `compact-view`
  and for every verb that writes, to the decision that is the opposite of it: one
  whose own title says every word asked about outright, outside any negation. The
  title is what states what was decided; a rationale says "not" for many reasons
  that leave the decision itself positive. A negation in a title reaches the rest of
  its clause (a comma, semicolon, colon, bracket or dash ends it), in any spelling
  and in whole words ("Notion" is not "not"): "Decision links use title slugs, not
  slugs remembered per browser" says "decision links use title slugs" outright and
  denies only the rest. The opposite decision stays a close candidate and is never
  `Resolved`: its reason is the polarity, `polarity_mismatch: true` on the candidate
  in JSON and `polarity: question is negated; this decision is not` in `--summary`
  output where `missing:` would be. So "don't adopt Kafka", "didn't we adopt Kafka"
  and "do not adopt Kafka" never resolve to the decision "Adopt Kafka"; they resolve
  to "Do not adopt Kafka" when that exists (it says neither word outright), and
  close candidates are dropped when one does, as they are for missing words.
- A title that leaves a word of the question out is not the opposite of it, and
  neither is one that denies it. Polarity is never a reason to prefer a decision:
  a negated question is not answered by whatever decision has "no" or "not" in its
  title, and the decision asked about is not passed over because it states its
  answer positively ("status history lists only what the log gives the UI" for "why
  does the page not show when a decision was accepted?"). Those decisions compete on
  how much of the question their title, topic keys and recorded question carry
  (below), like the answers to any other question.
- `recall` ranks as if the negation were absent. Polarity only breaks a tie: among
  decisions equal on closeness, headline and rank tier, one whose title does not say
  every word outright comes before one whose title does, and a negated question
  never lets a close match outrank a full one. The negation is listed in `ignored_words`.
- A question with no negation behaves as before, whatever a decision's title says.
  A description made only of question words and negations ("why not") is searched
  as written, like "why did we", and is not read as negated.

**Close candidates.** A question often names a word the record never uses
("why did we *finally* move the demo cell..."). When no decision matches every
term, decisions matching more than half of them (and at least two) come back as
an `Ambiguous` list, ranked by fewest missing terms, then by how many of the
matched terms the decision's title or topic keys carry (below), then rank tier,
then recency, each with `missing_terms` naming what it lacks. For a verb that writes
(`disagree`, `supersede`, `move`, `retitle`, `ground`, a grounding premise), a
close candidate is never `Resolved`, even when there is only one: the caller
picks with `--pick`, `#N` or `--id`. When any decision matches every term, close
candidates are dropped. One matching word out of two is not close enough, so an
unrelated description is still `NotFound`.

**Close candidates for a verb that only reads.** `why`, `verify`, `chain` and
`compact-view` resolve with `resolve_decision_for_reading`. A wrong pick there
shows the wrong decision, labelled with what it lacks; it writes nothing. Two
differences from a writer, both for a description no decision matches in full
(hivemind-3lko):

- The bar for a close candidate is `recall`'s: at least half of the terms. A
  question `recall` answers is therefore never answered "no decision matches"
  by `why`.
- When close candidates are all there is, and one is closer than the next and
  shares at least two terms, it is `Resolved` and carries its `missing_terms`.
  (So is a promoted close candidate that leads the others, see *what the question
  is about* below, though a decision that holds every word exists.)
  The verb shows the decision and says what it lacks: a `close match:` line
  ahead of `--summary` output, `close_match: {decision_id, title,
  missing_terms}` beside `data` in `--json`, the HTTP routes and MCP. Equally
  close candidates stay an `Ambiguous` list (rank and recency say nothing about
  which of two decisions was meant), and a candidate that shares one term is
  listed, not answered with.

*Closer* is two counts, compared in order. A candidate that lacks fewer terms is
closer. Between candidates that lack the same number, the one whose title or topic
keys contain more of the terms it did match is closer: a decision about a thing
says it in its headline, and one that merely holds the same words somewhere in a
long rationale or in its evidence is a weaker answer. "Is the product still called
Upheld" is lacked by two decisions, one without "called" and one without "upheld":
the decision titled "Name the product Upheld" has both of its words in the title,
the one that says "product" and "called" in its rationale has neither, so the
first is the answer. The basis is where the matched words sit (the `matched_fields`
a result already shows), and it only separates candidates that lack the same number
of words: it never lets a decision lacking more words win. The ordering applies to
every close list the resolver returns, including the one a writer gets, though a
writer still never has a close candidate picked for it.

**What the question is about** (hivemind-eral). Holding every word is not the same as
being the decision asked about. A few decisions have long rationales and long
evidence, and between them they hold nearly every common word, so for a plain
question they match in full whatever it asks ("is the product still called HiveMind"
is matched in full by a decision on a triage gate, whose rationale says all three),
and a full match drops every close candidate. The decision that is about the
question says it in its title and topic keys, lacks one word, and used to be dropped.
So where the words sit is compared before whether every word is somewhere:

- A decision's *aboutness* is the weight of the question's words that its title, a
  topic key or the question it records holds, as a word or a form of one (never inside
  a longer word: "show" is not "showcase", and never through a stand-in). A word weighs `ln((n + 1) / (holders
  + 1/2))` over the `n` decisions searched, `holders` being the decisions that hold it
  as a word anywhere, exactly as for a decision below the bar (§1.1): the one rare
  word of a question outweighs the generic ones around it. Between equal weights, the
  decision whose own title holds more of them is the one about the question: the
  title states what was decided, a topic key only files it. The decision whose id or
  title is the question, or that records the question as asked, is about it more than
  any other and is never put behind one.
- The decisions that hold every word (those of the right polarity, when any do) set
  the reference. A close candidate whose aboutness is strictly greater than every one
  of theirs is *promoted*: it is listed before them, and a verb that only reads
  answers with it, as for any close candidate (it shares at least two terms and leads
  the other promoted ones), naming what it lacks in `close_match`. A verb that writes
  never picks it; it gets an `Ambiguous` list with the promoted candidate first. A
  close candidate of the opposite polarity is never promoted. Close candidates that
  are not promoted are dropped when a decision holds every word, as before.
- Among the decisions that hold every word, the one whose title and topic keys carry
  more of the question's words comes first, then the one that leans on fewer stand-in
  words, then the rank tier. Two that carry the same weight are still equals, and
  the ambiguity gate applies to them as before. This replaces "the rank tier alone"
  for full matches: two decisions that each have one word in their title used to be
  equals however many of the other words each held in its title, and now are not.
- `recall` orders its list the same way (promoted close matches first, then the full
  matches by aboutness), so `why` names what `recall` just named.

A question that matches no decision's headline has no aboutness to compare and is
ordered as before. The basis is the decision's own words and the ledger's own counts;
nothing is learned and no word list is involved.

A full match that nothing out-heads is unchanged for both, and adds no `close_match`.

**A decision recorded more than once is one decision** (hivemind-83cj). When two
records are linked `SAME_AS` (the classifier named the earlier one when it
recorded the later, or `restatements apply` linked records made before that),
both resolvers and `recall` treat them as one: the earliest record that matches
the words is the candidate, and the other records come back as
`also_recorded_as: [{decision_id, title}]` (an `also recorded as:` line ahead of
`--summary` output, the key beside `data` in `--json`, the HTTP routes and MCP;
`recall` carries it on the item). The links are followed in either direction and
transitively, and the matches of the records are combined: the words a candidate
lacks are the words none of its records has. Nothing folds on closeness: two
records with no link between them stay two, and a description matching both is
`Ambiguous` as before (the tenv.1 rule). `--id` looks up exactly the record
named, and `search` lists records, not decisions. A `relation.removed` for a
`SAME_AS` is a ledger fact the graph does not project, so a retracted link is
still folded.

**`recall` asks the same way.** `recall_decisions` (Layer 3) drops the question
words ("what did we decide about projects" searches for `projects`) before it
searches (the same list, decision verbs and framing adverbs included), reports
them as `ignored_words`, and treats a question made only of question words as no
text filter, so `--topic` alone decides.

It then matches the way the resolver does, with a lower bar for close matches.
A word also matches a field word with the same stem (`moving` finds `moves`;
whole-word equality, as above), and, for `recall` and for the verbs that only read
(`why`, `verify`, `chain`, `compact-view`), two more kinds of word, so a question
asked with another form or a synonym of a decision's words still finds it
(hivemind-md0y):

- **The noun or the verb of a word.** A noun suffix is taken off (`accepted` finds
  `acceptance`, `refuted` finds `refutation`, `moved` finds `movement`, `secure`
  finds `security`; at least four or five letters must remain, so `section` is not
  `sect` and `former` is not `form`), and a verb in `-d`/`-de` meets its noun in
  `-sion` at the part they share (`superseded` finds `supersession`, `decide` finds
  `decision`, `expand` finds `expansion`). Still whole-word equality of a key, never
  a prefix, so `string` does not match `strategy`.
- **A stand-in word.** `WORD_GROUPS` in `src/queries/terms.rs` lists the few groups
  of words that say the same thing in this product's own vocabulary: `supersede` and
  `replace` (the site draws a supersession as "replaces"), `assumption`, `hypothesis`
  and `premise`, `ui` and `interface`, `graph`, `diagram`, `chart` and `picture`,
  `site` and `website`, `link`, `url` and `address`, `refute` and `wrong`, and
  `browser`, `laptop` and `phone`. The list is fixed and literal, like the question
  words: nothing is learned, and a word belongs in it only when people ask about the
  same decision with either. A stand-in is never the word itself: among decisions that
  lack the same number of words, the one that matches fewer of them only through a
  stand-in comes first (after the one whose title or topic keys carry more of the words), so
  among decisions that match equally well, one that says "interface" outranks one that says
  "UI" for a question about the interface.

A verb that writes (`disagree`, `supersede`, `retitle`, `ground`, ...) keeps the
resolver's own match, the word and its inflections: a synonym never picks the
decision a write lands on. A possessive is the word it belongs to ("the website's
picture" asks about the website).

A decision matching every word comes first (the one whose title and topic keys
carry more of the words first, then the one that leans on fewer stand-in words,
then rank tier, then decision id), except that a close match that carries more of
the question in its title and topic keys than any of them comes before them
(*what the question is about*, above); then,
fewest missing words first (then the decision whose title or topic keys carry more
of the matched words, as for `why` above, then the one that leans on fewer stand-in words,
then rank tier, then decision id), a
decision that lacks some of the words is still returned when it matches at least
half of them.
Each such close match carries `missing_terms` (`missing=` in `--summary`, and a
`Close matches` line in the digest naming what each one lacks), so a partial
answer never reads as a complete one. The bar is half, not the resolver's "more
than half, at least two", because `recall` only reads and labels what it returns:
"what did we decide about sign-in and pricing" finds the decision about either
word after the one about both.

A decision that holds fewer than half of the words is also returned, after every
decision at the bar, when the words it does hold are rare among the decisions
searched. A long question names one thing among generic words ("CLI query xpath css
text extraction"), and one rare word of six would never reach half. A word weighs
`ln((n + 1) / (holders + 1/2))` over the `n` decisions searched, `holders` being the
decisions that hold it as a word or a form of one; a word nobody holds weighs the
most, so a question that is mostly about what the ledger lacks never passes. A
decision below the bar is returned when

- it holds at least three of the words, or two that its title or topic keys hold,
  and together they weigh at least 1.25 times `ln(n + 1)` and a fifth of the weight
  of all the question's words: that many words few decisions hold do not meet by
  chance, whereas two that sit in a long rationale do ("load" and "timeout" of a
  load balancer question), or
- the ledger is small (16 decisions or fewer, too few for counts to say what is
  rare), and a word its title or topic keys hold is held by no other decision: its
  capturer named it as the subject.

At most three such decisions are added (the ones holding the most weight, then
decision id), each carrying `missing_terms` like any close match. `why` and
`verify` read the same list. There is no word list and nothing learned: the counts
are the ledger's own, so this part of recall, unlike the ordering of close
candidates above, does count words across the ledger. A question that shares no
rare words with a decision is still an empty answer.

A synonym the ledger never uses ("dotted" for "dashed", "emoji" for "reaction")
cannot be found this way: no word of the question is in the decision, and recall
does not guess.

`search` keeps every word, as a literal substring. When that finds nothing for a
phrase of three or more words it asks again as `recall` does, and answers with the
same list, so `search` and `recall` agree on a phrase written in an agent's own
words; a shorter query stays exact. `data.query` is the query as it was given and
each item carries `missing_terms`.

### 1.2 Recency, for free

Every node carries `event_origin` — the ledger offset at creation
(`origin_properties`, `src/projector/mod.rs:423-429`; merged onto every
`Decision` node by `project_decision_proposed`, `src/projector/mod.rs:485`).
`event_origin` is monotonically increasing, identical in meaning on both
backends, and is already used elsewhere as a recency proxy (`gap_events` in
`OutcomeReason::SupersededBy`, `src/queries/outcome.rs:40`). The resolver
reads it as a plain `Decision.event_origin` graph property (one extra column
in the existing `node_rows(graph, NodeKind::Decision)` scan,
`search.rs:578`) — no ledger access, no new backend-specific code path.

There is **no wall-clock timestamp on the projected `Decision` node today**
(only `event_origin`/`source`/`source_ref`/`tenant_id`, per
`origin_properties`). `decision_proposed_at_by_id` (`search.rs:462`) reads a
real timestamp, but from the SQLite ledger directly — not backend-agnostic,
not available for Postgres. This is a real gap for the output contract's
"when" field (§4) and is called out as Open Question 1.

### 1.3 Primitive contract

```rust
// src/queries/resolve.rs (new)

/// One candidate returned by the resolver, ordered by descending confidence.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ResolvedCandidate {
    pub decision_id: String,
    pub title: String,
    pub rank: u8,              // reused tier from evaluate_search_match; 0 = exact
    pub event_origin: i64,     // recency tiebreak, ascending rank order first
    pub matched_fields: Vec<String>,
}

/// Outcome of a resolve-by-description call: either a confident single
/// match, or a candidate list the caller must disambiguate.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ResolveOutcome {
    Resolved { candidate: ResolvedCandidate },
    Ambiguous { candidates: Vec<ResolvedCandidate> },
    NotFound,
}

pub fn resolve_decision_by_description(
    graph: &impl GraphView,
    description: &str,
    topic_hint: Option<&str>,   // optional --topic narrowing, same param search already accepts
) -> Result<QueryResponse<ResolveOutcome>>;
```

`ResolveOutcome` is a **value**, not an `Err`. Ambiguity is an expected,
common outcome (most descriptions will match more than zero decisions early
in a ledger's life) — modeling it as an error would force every caller into
exception-flavored control flow for the normal case, and would contradict
the project's own stance that `contested` is a status, not an error
(AGENTS.md §6). `NotFound` is likewise a value: zero matches is informative,
not exceptional.

### 1.4 Ambiguity gate — confidence margin

Given the sorted candidate list (primary key: `rank` ascending, secondary
key: `event_origin` descending — newest wins ties), define:

- **Resolved** iff there is exactly one candidate at the best `rank` tier,
  **or** the best-tier candidate's `event_origin` is strictly newer than
  every other candidate sharing that tier by more than a fixed margin (see
  Open Question 2 for the margin value — this needs a real number, not a
  polecat's guess, because it is the one knob that trades recall against
  silent-wrong-pick risk).
- **Ambiguous** otherwise: two or more candidates share the best rank tier
  with no clear recency separation. Return the full candidate list (capped
  at `MAX_QUERY_RESULTS`, `shared.rs:12`), each numbered `#1..#N` in the
  order returned (§5).
- For **write verbs** (`disagree`, `supersede`) the gate is strict: even a
  single-tier-apart near-tie forces `Ambiguous`. Read verbs (`why`,
  `compact-view`, `review`) may be slightly more permissive since a wrong
  read is cheap to notice and re-query; a wrong write is not. This
  asymmetry is deliberate and should be confirmed at the checkpoint (Open
  Question 3).

**Where the words sit comes before the rank tier** (hivemind-eral). Among the
decisions that hold every word, "the best rank tier" is the last of three keys: first
how much of the question the decision's title and topic keys carry (*what the question
is about*, §1.1), then how few of the words it matches only through a stand-in, then
the rank tier. Resolved still means exactly one candidate holds the best of all three,
with no numeric margin; two decisions that carry the same words in their headline and
share the rank tier are still `Ambiguous`. Two that each had one word in their title
used to tie however many of the other words each held in its title; they no longer do.

`--pick N` selects candidate `#N` from the *immediately preceding* resolver
call in the same invocation (used together with `--from-last`, §5) or is
rejected with a clear error if there is no prior candidate list to pick
from. `--id <decision_id>` bypasses resolution entirely — the existing
`decision_node_exists` check (`shared.rs:15`) becomes the validation path
for both the new positional-description mode and the legacy `--id` mode, so
`--id` behavior for existing scripts/callers is unchanged.

### 1.5 Backends

`resolve_decision_by_description` (`src/queries/resolve.rs`) — and every
fluent verb built on it: `situational` (`src/queries/situational.rs`),
`why`/`get_decision_neighborhood` (`src/queries/neighborhood.rs`),
`verify`/`get_decision_outcome` (`src/queries/outcome.rs`), and the
resolution step in front of `disagree`/`supersede` — takes only
`graph: &impl GraphView`. Nothing in that path is SQLite- or Postgres-
specific: it runs unmodified against the SQLite-projected `MemoryGraph` and
against `PostgresGraphView` (`src/projector/postgres/tests.rs` asserts
parity between the two for the resolver and its callers). This is what
"backend-agnostic" means here — one code path, no `#[cfg]`, no backend match
arm, for every fluent verb except `search`.

`search` is the one place backend choice is visible, and only at
the ranking layer: `search_decisions_any` (`src/queries/search.rs`)
dispatches SQLite to FTS5 (`search_decisions_fts_with_context`, backed by
the `decision_search_fts` virtual table) and Postgres to a portable
in-memory term matcher (`search_decisions_with_ledger`) reusing the same
`collect_graph_search_results` tiering the resolver uses, since Postgres has
no FTS5 equivalent available. Both paths converged on one ordinal ranking
scheme and return identical order for identical fixtures (see
`docs/SEARCH_DESIGN.md`'s Ordering Guarantees), so a caller holding only an
`AnyLedger` — the CLI, stdio MCP, and the HTTP `/v1/decisions/search` route —
never needs to know which backend it is on.

`recall` does not dispatch: FTS5 matches whole tokens, every one of them, which
is the strictness a question has to get past, so `recall_decisions` calls
`search_decisions_fluent` (`src/queries/search.rs`) on both backends. It is the
same in-memory matcher and tiering as `search_decisions_with_ledger`, with the
resolver's stemmed terms and close matches. Its digest and citation list, and its
order, are identical on either backend by construction.

---

## 2. Ambiguity Is Not an Error — Compliance Note (Principles 1 & 7)

Principle 1: "No smart behavior — no search, ranking, summarization, model
call, similarity, or recommendation — ever touches the write path"
(`PRINCIPLES.md:15`). Principle 7: "Layer 2 (query) does not call LLMs,
score, cluster, or rank" (`PRINCIPLES.md:82`).

Read literally, "does not... rank" appears to forbid exactly the mechanism
this design reuses. In practice the codebase already draws a narrower line:
`search.rs`'s `rank: u8` tiering and the SQLite FTS path's `bm25` score
(`search.rs:416`) are both deterministic, non-learned, non-probabilistic
functions of exact string containment and term frequency — no model call, no
embedding, no similarity metric, nothing that could disagree with itself
between two runs on the same data. That is the reading the bead text itself
takes ("deterministic: term match + topic + recency... no LLM — PRINCIPLES
1/7"). This design does not push that line further: it reuses the existing
tier system unmodified and adds one new deterministic field
(`event_origin`, an integer ledger offset, already graph-native). No
embeddings, no fuzzy/edit-distance matching, no learned weights are
introduced. Confirming this reading is correct is Open Question 4 — it is
the load-bearing constraint for the whole bead, and "the existing code
already does something adjacent" is not the same as "alex has signed off on
extending it."

---

## 3. Verb-by-Verb Contract

Positional free text follows the `recall` precedent exactly: `pub query:
Option<String>` as the first positional field on each `*Args` struct
(`QueryRecallArgs`, `args.rs:1007`), so existing flag-only invocations are
unaffected (the field is optional) and `--id` continues to work unchanged.

| Verb | Today | Change |
|---|---|---|
| `disagree` | `--decision <id> --reason <r>` (`args.rs:281`) | add positional `Option<String>` description; `--decision`/`--id` stays the escape hatch |
| `supersede` | `--old <id> --title ... --rationale ...` (`args.rs:290`) | add positional description resolving `--old`; rest unchanged |
| `move` (**new**, hivemind-s15q.11) | does not exist | positional description or `--decision <id>`, plus `--to <project>` `[--pick N] [--topic T] [--reason R]`; the same write gate as `disagree`/`supersede`; where the decision is now is read from the ledger, never typed |
| `ground` (**new**, hivemind-gwhr.4) | does not exist | positional description or `--id <id>`, plus `[--pick N] [--topic T]` and the capture grounding flags (`--rests-on-decision`, `--rests-on-evidence` with `--evidence-source`, `--rests-on-assumption`, `--bet`) and `--answers "<question>"` (hivemind-zdsh.16; may stand alone); the same write gate as `disagree`/`supersede`; says what an existing decision rests on, or which question it answers, after the fact |
| `get_supersession_chain` | `--id <id>` (`QueryDecisionArgs`, `args.rs:940`) | add positional description; **new alias** `chain` (friendlier name per bead item b) |
| `get_decision_neighborhood` | `--id <id> --depth --relations --compact` (`args.rs:959`) | add positional description; **new alias** `why` |
| `compact-view` | `--id <id>` (`QueryDecisionArgs`) | add positional description |
| `review` (**new**) | does not exist as a single-decision verb today | see §3.1 |

**`ground` and the capture verbs share one grounding vocabulary.** A decision
captured without saying what it rests on reads `rests on: nothing declared` in the
brief (§4). `hivemind ground "<description>"` adds the answer later: the target is
resolved with the strict write-verb ambiguity gate, and every premise named by
`--rests-on-decision` is resolved by the same resolver before the first write, so
an ambiguous or unmatched premise refuses the whole call with numbered candidates
and writes nothing. The capture verbs (`emit decision.capture`, `supersede`) take
the same premise flags, and there the requirement applies: a capture that names
nothing is refused. The grounding `ground` writes is append-only, attributed to
whoever ran it, and carries no link to the decision's proposal, so the brief shows
it as attributed **later** rather than **at capture**. `--confidence` is refused
by `ground`: it is the decider's own words at capture. A premise that already rests
on the target is refused, since it would close a loop.

### 3.1 The `review` naming collision

The bead lists `review` among the verbs to make fluent, glossing it as
"Verify: 'did that hold up?' by description" (parent hivemind-tenv,
item 4). But the CLI already has a `review` subcommand
(`ReviewArgs`, `args.rs:317`) that is a **bulk filter report** — actor glob
patterns, a `since`/`until` window, `--unreviewed-only` — with no
`decision_id` field at all. It answers "what decisions need review", not
"did this one decision hold up." The verb that actually answers "did that
hold up" is `get_decision_outcome` (`src/queries/outcome.rs:94`,
`DecisionOutcome.held_up` + structured `reasons`), which today is **MCP-only**
(`src/mcp.rs:1086`) with no CLI query subcommand at all.

**Recommendation:** add `get_decision_outcome` as a new CLI query subcommand
(alias `review`, matching the bead's naming and the parent's "did it hold up"
framing) accepting the same positional-description contract as the other
verbs. Leave the existing bulk `review` subcommand untouched — it is a
different, filter-shaped verb that was never a candidate for
resolve-by-description (it has no single target to resolve). This is Open
Question 5: confirm the naming does not collide confusingly once both exist
side by side (`hivemind review` = bulk report, `hivemind query review
<description>` = single-decision outcome check) — an alternate name
(`verify`, `held-up`) may be clearer as the query-subcommand alias.

### 3.2 `--pick N` and `--topic`

`--topic` is added as an optional narrowing filter on every fluent verb
(mirrors `get_relevant_decisions --topic`, `args.rs:950`, and
`search --topic`, `args.rs:981`) — passed through to
`resolve_decision_by_description` as `topic_hint`. `--pick N` is added to
every fluent verb per §1.4/§5.

### 3.3 Backward compatibility

No existing flag is removed or renamed. `--id` keeps its current behavior
and error semantics on every verb (`decision_node_exists`,
`shared.rs:15`, unchanged). Existing scripts, the `hivemind-capture` plugin,
and MCP callers that always pass `decision_id` are unaffected — this is a
strictly additive contract change.

### 3.4 MCP parity (status)

Shipped (`hivemind-ot72.5`–`hivemind-ot72.8`, via the shared `resolve_target`
core in `src/mcp/core.rs`): `get_decision_neighborhood` (why),
`get_decision_outcome` (verify), `disagree_decision`, and `supersede_decision`
each take an optional `description` string alongside their existing
`decision_id`/`old_decision_id`, plus an optional `topic` narrowing filter;
`move_decision` (`hivemind-s15q.11`) was built on the same core and takes the
same `decision_id`/`description`/`topic` selectors.
`decision_id` is simply left out of the tool's JSON Schema `required` array
rather than expressed via `oneOf` — the mutual-exclusion rule ("exactly one
of `decision_id`/`description`") is enforced at runtime, not in the schema,
and a call supplying neither returns a real MCP tool error (`one of
"decision_id" or "description" is required`) since a malformed call is the
one case that *is* exceptional here.

`ground_decision` (`hivemind-gwhr.4`) is registered on both transports and takes
the same `decision_id`/`description`/`topic` selectors plus a `grounding` array;
`capture_decision` and `supersede_decision` take the same `grounding` array, and a
`description` inside it (a premise decision) returns the same
`{outcome, field: "grounding[i]"}` shapes below.

**Still id-only over MCP** (no fluent parity yet): `get_supersession_chain`
(`hivemind-ot72.9`, open) and `hivemind_compact_view`
(`hivemind-ot72.10`, open) both still require `decision_id`.
`get_decision_context` and `get_decision` are not on the fluent-parity
roadmap at all — they stay id-only by design, since both are meant to be
called once a decision is already resolved (e.g. right after a fluent
`why`/`verify` call), not as a first fluent hop.

**No `--pick`/`#N` over MCP.** Unlike the CLI, MCP has no local per-session
state to cache a candidate list in (stdio is stateless per call; HTTP has no
session directory) — see `resolve_target`'s doc comment in
`src/mcp/core.rs`. An ambiguous or not-found description is returned to the
caller as data, and the caller re-calls with `decision_id` from the
candidate list once it has one.

**Ambiguous and not-found are both success, not errors** — per alex's
2026-09-21 decision ("errors are for the exceptional case; a miss is not
exceptional, it's data"): every fluent MCP tool wraps its result in the same
`{result_count, truncated, latency_ms, data: {outcome, ...}}` envelope the
CLI's `--json` output uses for `ResolveOutcome`, whether `outcome` is
`"resolved"`, `"ambiguous"` (with `candidates`), or `"not_found"`. None of
these is a JSON-RPC error or `isError: true` — an agent branches on
`data.outcome`, the same way it would branch on any other field, rather than
needing a separate error-handling path for "nothing matched." This is one
contract shared verbatim by CLI (`--json`), MCP, and the four fluent HTTP
routes (`docs/DEPLOYMENT.md`, `docs/SELF_HOSTING.md`) — the only surface
without it is the two still-id-only MCP tools above, and the HTTP write
routes (`disagree`/`supersede`), which remain `decision_id`-path-only and are
not fluent over HTTP at all today.

---

### 3.5 Reads past a row nobody can read (hivemind-qo11.10)

Every verb above reads a ledger that is append-only and shared, so it can hold a
`decision.scored` (assessment) or `decision.metadata_derived` row whose payload
fails validation: a buggy or old producer, a partial import, a hand edit. A read
does not fail over it. The projector and the history and search queries skip such
a row, and the decision it names answers with what its own events say (the floors,
no `model_assessment`). The answer says so: `notice` beside `data` in `--json` and
in the MCP result, a final `notice:` line in `--summary` output (CLI `query`,
`digest`, `export`, `quality-scan`; every MCP read tool, stdio and `/mcp`),
naming the ledger events. A ledger with nothing unreadable in it carries no
`notice`. Only these two annotation kinds are skipped: a malformed event that the
graph is built from still fails the read.

## 4. Output Contract — `DecisionBrief`

No existing query response leads with "the decision, then why, then who,
then does it still hold" — the pieces exist but are scattered across three
separate query functions:

- `get_decision` (`decision.rs:37`) → `DecisionView`: id, title, rationale,
  `option_ids` (ids only, **no labels** — see below), `chosen_option_id`,
  status, evidence/hypothesis ids.
- `get_decision_context` (`context.rs`) → `DecisionContext`: `proposer_id`
  (who *recorded* it), `accepted_by` (who actually *decided* — may differ
  from `proposer_id`, or be empty when unreviewed; hivemind-zdsh.9),
  `source`, `source_ref`, `review: ReviewShape` (unreviewed / self_accepted /
  peer_reviewed / disputed), and `delegated_by` (the human whose delegated
  scope an agent's self-acceptance fell within; absent when the agent decided
  alone — hivemind-zdsh.6) — this is "who decided."
- `get_decision_outcome` (`outcome.rs:94`) → `DecisionOutcome`: `held_up`
  plus structured `reasons` (superseded / stale premises / contested) — this is
  "STILL-HOLDS." How the decision was made (options weighed, what it rests on)
  is quality, not outcome: `score_decision` reports it.

**Known gap:** `option_ids` are ids, not labels. Option node `label` is a
real graph property (`opt_props.insert("label", ...)`,
`src/projector/mod.rs:1052`) reachable via `node_rows(graph,
NodeKind::Option)` (already scanned by `collect_graph_search_results`,
`search.rs:581`), but no existing query surfaces it. There is also **no
per-option rejection rationale** anywhere in the graph schema — only
`ForeclosedOptionAttribute.description` (`events.rs:521`), which belongs to
the classifier/ingest extraction pipeline, not the core `Decision`/`Option`
model that `get_decision` reads. "REJECTED options + why" therefore means:
resolve each non-chosen `option_id` to its `label` (new lookup, small
addition), and reuse the single decision-level `rationale` string as the
shared "why" for the choice among options — there is no way to show a
*distinct* rejection reason per option today without a capture-side schema
change, which is out of scope. This is stated as a limitation in the
rendered output, not silently glossed over (AGENTS.md §6: no invented
confidence).

```rust
// src/queries/resolve.rs or a new src/queries/brief.rs

pub struct DecisionBrief {
    pub decision_id: String,
    pub title: String,
    pub project: String,                      // address: a registered handle or the recorder's derived personal address
    pub project_label: String,                // what a person calls it: display name, handle, or "alex's personal project"
    pub rationale: String,
    pub question: Option<String>,             // the question it answers, in the capturer's words (a decision linked later reads the question node's words)
    pub question_id: Option<String>,          // the shared Question node it answers (ANSWERS); absent when it names none
    pub other_answers: Vec<QuestionAnswer>,   // other decisions answering the same question, in event order, each with status + chosen option
    pub chosen_option: Option<OptionLabel>,
    pub rejected_options: Vec<OptionLabel>,   // label-resolved, rationale shared not per-option
    pub decided_by: DecidedBy,                // proposer_id, decider_ids, source, source_ref, delegated_by, review shape
    pub rests_on: Vec<GroundingItem>,         // what it rests on: prior decision / evidence / assumption / bet, each with state + provenance
    pub grounding_state: GroundingState,      // grounded | bet | nothing_declared ("never asked")
    pub expressed_confidence: Option<String>, // low | medium | high, the decider's words at capture
    pub dependents_count: usize,              // decisions that follow from this one
    pub still_holds: StillHolds,              // held_up + reasons (+ unchecked bets), from DecisionOutcome
    pub topic_keys: Vec<String>,
    pub status: DecisionStatus,               // proposed/accepted/contested/superseded
}

pub struct OptionLabel { pub option_id: String, pub label: String, pub recorded_as: Option<String> } // recorded_as: what the capture recorded, when `label` reads it differently (slug -> words); absent otherwise
pub struct DecidedBy { pub proposer_id: Option<String>, pub decider_ids: Vec<String>, pub source: String, pub source_ref: Option<String>, pub delegated_by: Option<String>, pub review: ReviewShape }
pub struct StillHolds { pub held_up: bool, pub reasons: Vec<OutcomeReason>, pub unchecked: Vec<UncheckedBet> }
```

`get_decision_brief` composes the three existing query calls plus the new
option-label lookup — it does not duplicate their logic, and it stays
`Layer 2`: three deterministic graph reads, no write, no LLM.

**Which question it answers** (hivemind-zdsh.16). Decisions whose question is
the same after lowercasing, collapsing spaces and dropping trailing punctuation
share one `Question` node. The brief prints `answers: <question>` and one
`also answered by: <title> [status] (chose <option>)` line per other decision
that answers it, a superseded answer included. Two accepted, non-superseded
decisions that chose different options carry `conflicting_answer` in
`still_holds.reasons` on both. It is attention, not staleness: `held_up` is
unaffected and neither answer is resolved for the reader. `situational` lists
the accepted answers recorded after a matched decision as `newer_answers` (text:
`answer<TAB>newer<TAB><id><TAB><title> [accepted] (chose ...)`). The pure read
`answers_to(question_id | text)` lists every answer to one question in event
order.

**What it rests on** (hivemind-zdsh.15 §5, hivemind-gwhr.3). `rests_on` answers
"what does this decision rest on?" from the graph: a prior decision
(`FOLLOWS_FROM`), an observation (`BASED_ON`), an assumption or a declared bet
(`ASSUMES`, hypothesis kind `assumption` | `bet`). Each item carries its own
state — a decision `holds` / is `superseded` / `rejected` / `contested`; an
assumption is `open` / `supported` / `refuted`; a bet is `open` (with its check
date, and whether it is overdue) / `held` / `failed` — and whether it was named
**at capture** or attributed **later** (by whom, when), read from the edge's
causing proposal event, never blended. Staleness is never silent: a prior
decision that was superseded or rejected adds `PremiseSuperseded` /
`PremiseRejected` to `still_holds.reasons` and flips `held_up` (like a refuted
assumption); a contested premise is shown but does not flip it; an overdue bet
is reported under `still_holds.unchecked` (attention, not staleness) and does
not. `grounding_state: nothing_declared` means "never asked" (legacy records,
classifier extraction, document import, raw `emit decision.proposed`) and is shown
as "rests on: nothing declared" (a premise link, evidence, an assumption or a
declared bet all count as declared). It is not a reason the decision stopped
holding: `still_holds.reasons` lists only what happened to it (superseded, a stale
premise, contested), and how well it was made is the quality profile's
(`score_decision`). Text renderers show labels and keep ids in the JSON.

Every fluent verb's output, on success, leads with a `DecisionBrief` (or a
list of them for multi-decision verbs like `get_supersession_chain`). JSON
is the default (matches `QueryArgs`'s existing convention,
`args.rs:887,889`); `--summary` renders compact text through the existing
generic dispatcher `format_query_response` (`render.rs:171`), following the
established pattern of a per-verb `render_*_summary(&T) -> String` closure
(e.g. `render_neighborhood_summary`, `render.rs:342`) — but as short
labeled paragraphs (decision / rationale / rejected options / recorded by
and decided by, shown as one line on self-acceptance or two when the
recorder and decider differ / still holds), not the tab-cell table style
used by list-shaped commands
(`summary_cell`, `render.rs:456`), since a `DecisionBrief` is a single
record meant to be read, not a row in a list. IDs appear only in a trailing
"ref: <decision_id>" line — present for follow-up (`--id`, `--pick`), never
required reading to understand the answer.

**`why` reuses the brief.** `get_decision_neighborhood` (`why`) returns the
one-hop graph and leads with this same `DecisionBrief`: `root` carries it
flattened in beside `id` (title, rationale, chosen and rejected option
labels, decided-by, still-holds, status), and every non-actor node carries a
`label` — a decision's title, an option's label, a hypothesis' statement, an
evidence item's content (clipped to 200 characters with a trailing `…`).
`why --summary` prints the brief block first, then the graph as
`root`/`node`/`edge` lines. `compact-view` reads the bare structure without
the brief or labels.

**`why` carries the decision's timeline.** Where the caller holds the ledger
(the CLI, the stdio server, the HTTP endpoint), `why` also returns
`timeline`: the decision's dated story from the ledger, oldest first, each
entry cited by its event (asked, recorded, accepted or rejected, superseded,
retitled, moved, a premise no longer standing), with `asked_at`, `decided_at`
and `asked_to_decided_seconds`. The graph holds no per-edge times, so a
graph-only `get_decision_neighborhood` leaves it absent. See
[`ATTENTION_LISTS.md`](ATTENTION_LISTS.md), which also describes the waiting,
contested and changed lists.

**Every answer names its project.** Every decision an answer returns carries
`project` (the address: a registered handle such as `billing`, or the
recorder's derived personal address such as `personal:human:alex`) and
`project_label` (what a person calls it: the display name the project was
registered with, else its handle; for a personal address the person or agent
tool it belongs to — "alex's personal project", "claude agents' personal
project"); a decision that was moved names the project it moved to. This holds
for `get_decision`, the brief (`verify`), the outcome, the neighborhood (`why`,
on the root and on each decision node), the compact view, `search`, `recall`,
`situational`, `recent` and the decision log. The summary renderers show the
label: a trailing `project=<label>` field on the tab-separated rows, a
`project: <label>` line on the paragraph-shaped ones (brief, compact view,
digest, decision log). A decision that was only named
by a decision request or a blocker before any proposal recorded it has no
project: `project` is `null` and the label says "no project recorded", so an
unassigned decision is visible rather than blank or guessed. Labels are read
from the registered projects, never inferred (Layer 2 only).

**`situational` is project-first.** `query situational --project <handle>` (MCP
`get_situational_decisions` argument `project`) asks from one project: a
registered handle or a personal address. An unregistered handle is refused with
the `hivemind project register` hint, never answered as an empty result. The
answer holds only decisions filed under that project, then the project it is
`part_of` (inherited constraints), then the projects it `depends_on`, in that
order (each group keeps the usual score order). Each match carries a `scope`
label: `own project`, `from Platform; Billing is part of it`, or `from Auth;
Billing depends on it`; `--summary` prints it as a trailing `scope=<label>`
cell. Staleness is unchanged and applies across the hop: a superseded Auth
decision is labelled superseded in Billing's answer. The answer also carries a
`scope` note (a `scope` line under `--summary`, also on an empty answer) that
names every project looked in and states where the walk stopped: how many more
levels up the `part_of` chain, and how many more linked projects, were not
followed. Each link is followed one hop; the counts are what a later "go
deeper" step would reach, and a project's children and dependents are never
visited. Links are read from the ledger, so a retracted link is not followed.
Without `--project` the whole tenant is searched and the answer is unchanged.
The REST `GET /v1/decisions/situational` route does not take `project`.

**`recall` is project-first too.** `query recall <question> --project <handle>`
(MCP `recall_decisions` argument `project`) asks the same way and reuses the
same project structure, so the answer holds only the project's own decisions,
then the parent's (inherited constraints), then one hop over `depends_on`, in
that order; within a group the usual ordinal rank order applies, on SQLite's FTS
and on Postgres's portable matcher alike (the project is a filter on the
decision, applied after the text match, not a walk per result). Each ranked item
carries the same `scope` label (`own project`, `from Platform; Billing is part
of it`, `from Auth; Billing depends on it`), and the response carries the same
`scope` note naming the projects looked in and how far the walk stopped, also on
an empty answer; `--summary` prints a `scope` line and a trailing `scope=<label>`
cell on each scoped row. Staleness is unchanged across the hop, an unregistered
handle is refused with the register hint, and without `--project` the answer is
exactly what it was. The digest is built from the same decisions in the same
order; it does not repeat the labels. The REST `GET /v1/decisions/recall` route
does not take `project`.

**The project is always explicit on a read.** Neither `situational` nor `recall`
(nor any other query verb) works out a project from the working directory or the
rig: only the capture verbs have `--project-from-context`. Ask project-first by
passing `--project <handle>` (for a personal project, its address, for example
`personal:human:alex`, which has no links, so the scope note names only itself).
A decision with no recorded project, or one filed in a project outside the scope,
is left out of a scoped answer; the scope note lists what was looked in. The
`hivemind-context` plugin's read commands forward their arguments, so
`/hivemind-context:situational --project billing` works, and without the flag
the whole tenant is searched as before. Over HTTP the argument is the same: MCP
`get_situational_decisions` and `recall_decisions` take `project`; the REST
routes do not.

**`move` puts a decision in the right project.** `hivemind move "<description>"
--to <handle>` (or `--decision <id>`, `[--pick N] [--topic T] [--reason R]`) and
MCP `move_decision` (stdio and HTTP, one core function) resolve the decision the
way `disagree` and `supersede` do, with the strict write gate: a description that
matches several decisions returns the numbered candidates and writes nothing
(`--pick N` or `#N` settles it), and one that matches none is a successful reply,
`{"outcome": "not_found"}`, never an error. A move names only the target. Where
the decision is now is read from the ledger, so a caller never types it. A
registered handle or the actor's own personal address is accepted; an
unregistered handle is refused with the register command, another actor's personal
address is refused, and a decision already in the target is refused plainly. The
reply is the recorded fact:

```text
$ hivemind move "quarterly release trains" --to platform --reason "release cadence is a platform-wide rule"
event_id=37 decision_id=decision-1a81... from=personal:agent:claude to=platform
```

`--json` and MCP return `{decision_id, event_id, from, to, reason?}`. Afterwards
every answer shows the new project and `project_source` reads `moved`. The
history keeps every move: `query get_recent_activity` and
`get_decisions_changed_since` return a `project_moved` row per move with the two
ends (`project_move {from, to, reason?}` in JSON, `moved=<from>-><to>` under
`--summary`), the actor, and the time. Reversal is another move with the
ends swapped, and nothing is edited or deleted. `verify` and `why` show the
decision's current project, not its moves.

**A project's own list.** `hivemind project decisions <handle-or-personal-address>`
lists the decisions filed under one project, oldest first, paged, with `truncated`
and a cursor (`--limit`, `--cursor`); an unregistered handle is a successful reply,
never an empty list: `outcome: not_found` in JSON, and in text a line naming the
`hivemind project register` command. On a personal address it is the
review list, headed `in human:alex's personal project, not yet shared: N`: every
session of one agent tool lists together (`personal:agent:claude`), and each row
shows its `session` and how its project was determined. Nothing moves on being
listed; picking a decision from it and moving it is the review.

**The decision log is grouped per project.** `hivemind export --format markdown
--out <dir>` writes `INDEX.md` with one section per project, and per project a
`projects/<handle>/INDEX.md` plus one file per decision; personal projects go
under `projects/personal/<actor>/`. `--project <handle-or-personal-address>`
exports one project, and an unknown handle writes nothing and reports
`outcome=not_found`. Each decision file ends with Outcome (what happened to it:
superseded, premise gone, contested), Quality profile (the seven dimensions of
`score_decision`, each with its level, reasons and ids, or why it was not assessed)
and Provenance.

---

## 5. Short-Handle Continuation

No existing precedent (`resolve`, `ambiguity`, `--pick`, `candidate` do not
appear anywhere in `src/` outside `suggest.rs`'s unrelated document-extraction
candidates, per grep — this is new ground).

**Design:** every resolver-backed response — both `Ambiguous` candidate
lists and `Resolved` single answers — writes its candidate set to a
session-local file: `--from-last <path>` (default
`$HIVEMIND_LAST_CANDIDATES` env var if set, else a fixed path under the
CLI's existing config/state directory — same directory class already used
for local ledger state, not `/tmp`). The file holds the numbered candidate
list (`decision_id` + title + rank, JSON) from the most recent resolver
call. A subsequent invocation's `#N` positional argument (recognized by a
`#` prefix, distinct from a free-text description) reads that file, picks
entry `N`, and resolves directly — equivalent to `--pick N` but addressable
across separate CLI invocations within a session, which `--pick N` alone
(single-invocation only, per §1.4) cannot do. The same `#N` names a premise:
`--rests-on-decision '#N'` on `decision.capture`, `supersede` and `ground` picks
candidate N of the previous ambiguous list, which is how a refused capture is
re-run.

This keeps the mechanism simple: no server-side session state, no new
ledger writes (continuation state is not decision memory — it's UI
convenience and explicitly does not belong in the ledger per AGENTS.md §2,
"not a chat archive or conversational memory store"), one file, overwritten
on every resolver call. Concurrent sessions on the same machine sharing the
same file is an accepted rough edge for v1 (Open Question 6: is a
session/PID-scoped filename needed, or is "last resolver call in this
directory" fine for a single-engineer-plus-agents tool per the VISION.md
"who it's for" framing?).

---

## 6. Test Plan

### 6.1 Resolver + ambiguity gate (new)

Unit tests in `src/queries/tests.rs` style (in-process `MemoryGraph`,
synchronous, no I/O) covering: exact-title resolve, exact-id resolve,
single-term substring match with no competitor (Resolved), two decisions
sharing a title substring with no recency separation (Ambiguous, both
listed, numbered), the same case with the write-verb strict gate applied
(still Ambiguous even where the read-verb margin would resolve it, once
Open Question 3 sets the actual thresholds), `--pick N` out of range
(clear error, not a panic), `#N` with no prior candidate file
(clear error), zero matches (`NotFound`), and `--topic` narrowing a
multi-match description down to one.

### 6.2 Dual-backend parity (existing precedent, extended)

`src/projector/postgres/tests.rs` already runs `crate::queries::{get_decision,
get_supersession_chain, search_decisions}` against both `MemoryGraph` and
`PostgresGraphView` via `with_postgres_graph` (`tests.rs:227`, gated on
`HIVEMIND_TEST_POSTGRES_URL` and the `shared-backend-postgres` feature,
skips cleanly when the env var is unset). `resolve_decision_by_description`
and `get_decision_brief` are added to that same parity test — same fixture
ledger, same ranked-candidate assertion, both backends, one test body. This
is the established pattern in this codebase for dual-backend query
coverage; no new test harness is needed.

### 6.3 Reference docs

Every new/changed CLI subcommand and flag needs its `--help` text updated and
the reference regenerated in the site repo so `cli.md` stays accurate — see
[QUALITY_GATES.md → Reference Docs](QUALITY_GATES.md#reference-docs-live-in-the-site-repo).

---

## 7. Open Questions for Alex

1. **`event_origin`-as-recency vs. a real timestamp.** `event_origin` stays
   canonical for the resolver's *ranking* — that matches existing doctrine
   ("Offset bounds are canonical. Timestamp bounds are resolved to concrete
   event offset bounds", `SEARCH_DESIGN.md:184`) and needs no design change.
   The open question is narrower: should the projector *also* stamp
   `occurred_at` onto the `Decision` node (small, deterministic,
   Layer-1-compliant addition to `project_decision_proposed`,
   `projector/mod.rs:485`) purely so `DecisionBrief`'s "when" is a date an
   agent can read, rather than a bare ledger offset? Recommendation: yes,
   as a *display* field only, never as a ranking input — the output
   contract's "when" is explicitly required and an offset alone is a worse
   answer for a human/agent reading the brief, even though it stays correct
   for the resolver itself.

2. **Confidence margin value.** What recency gap (in `event_origin` units,
   or as "top candidate must be the only one at best rank" with no numeric
   margin at all) counts as "clearly ahead" for the ambiguity gate?
   Recommendation: start with **no numeric margin** — Resolved only when
   exactly one candidate occupies the best rank tier; any tie at the best
   tier is Ambiguous, full stop. This is the simplest rule that cannot
   silently pick the wrong one of two similarly-recent, similarly-matching
   decisions, and it can be loosened later (layer-3 candidate, not layer-2)
   if it proves too conservative in practice.

3. **Read vs. write gate asymmetry.** Confirm read verbs (`why`,
   `compact-view`, `review`) may use the same gate as write verbs
   (`disagree`, `supersede`), or should be strictly identical for v1.
   Recommendation: identical gate for all verbs in v1 (simpler, and matches
   §7.2's "no numeric margin" recommendation which leaves no daylight to
   asymmetrize anyway) — revisit only if it proves too conservative for
   read verbs in practice. Revisited for close candidates only (hivemind-3lko):
   the gate for a full match stays identical for every verb; what a read verb
   does with a description no decision matches in full differs (see "Close
   candidates for a verb that only reads", §1.1).

4. **Principle 1/7 reading.** Confirm the "deterministic tiering + ledger-
   offset recency, no embeddings/fuzzy matching/learned weights" reading of
   "Layer 2 does not... rank" (§2) is the intended one — this is the
   load-bearing constraint for the entire bead.

5. **`review` naming collision.** New CLI query alias for
   `get_decision_outcome` — keep `review` (matches the bead's own wording,
   collides in spirit with the existing bulk `review` report subcommand) or
   use a different alias (`verify`, `held-up`)? Recommendation: `verify`,
   with `review` left free for the existing bulk-report meaning, since
   having `hivemind review` (bulk) and `hivemind query review <desc>`
   (single-decision) coexist under the same word is exactly the kind of
   ambiguity this whole bead exists to eliminate for agents.

6. **Continuation file scope.** Single fixed path (last resolver call,
   process-global) or session/PID-scoped? Recommendation: fixed path for
   v1, matching the single-engineer-plus-agents scope in VISION.md.

7. **`get_supersession_chain` alias name.** Bead item (b) asks for "a
   friendlier alias" but does not name it. Recommendation: `chain`.

---

## 8. Layers Compliance Check

| Concern | Layer | Verdict |
|---|---|---|
| `resolve_decision_by_description` (term match, rank tiering, recency) | Layer 2 | Deterministic graph read reusing `evaluate_search_match`'s existing tiering; no LLM, no learned weights |
| Ambiguity gate | Layer 2 | Pure comparison over already-computed ranks/`event_origin`; returns a value, never throws |
| `get_decision_brief` | Layer 2 | Composes three existing Layer-2 query functions + one new deterministic node-property read (`Option.label`) |
| `--pick N` / `#N` continuation file | CLI-layer UX, not a query or a ledger write | Local convenience state only; explicitly not decision memory (AGENTS.md §2) |
| Optional `occurred_at` projector stamp (Open Q1) | Layer 1 write | Deterministic, no LLM — same shape as existing `event_origin`/`source` stamping |

No layer boundary is crossed. The write path (`disagree`, `supersede`) only
gains a *resolution step before* the write — the write itself
(`decision.disagreed`/`decision.superseded` event emission) is unchanged and
still keyed by a concrete `decision_id`, exactly as today, once resolved.

---

## 9. Summary and Next Steps

This design adds one shared, deterministic resolve-by-description primitive
(reusing `search.rs`'s existing rank-tiering, adding an `event_origin`
recency tiebreak and a conservative no-numeric-margin ambiguity gate),
extends six CLI verbs (plus their MCP equivalents) to accept free text with
`--id`/`--pick N` as escape hatches, introduces a shared `DecisionBrief`
output contract composing three existing query functions, and a simple
file-backed `#N` continuation mechanism. It surfaces one genuine schema gap
(no per-option rejection rationale) honestly rather than papering over it,
and one naming collision (`review`) that needs a decision before
implementation, not after.

**Immediate next steps (after alex sign-off):**
1. Alex resolves §7 open questions 1-7 (margin value, `occurred_at`
   stamping, `review` naming, and the Principle 7 reading in particular).
2. Implement `src/queries/resolve.rs` (primitive + ambiguity gate) and its
   dual-backend parity tests (§6.2).
3. Implement `get_decision_brief` and the `--summary` renderers.
4. Wire the six CLI verbs + `get_decision_outcome`'s new CLI subcommand +
   MCP tool parity (§3.4).
5. Reference regenerated in the site repo, full gate set, draft PR for CI validation.

Nothing in steps 2-5 begins before alex reviews §7.
