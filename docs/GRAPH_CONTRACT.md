# Graph Contract: Arrows Run Newer → Older

Every arrow a reader is shown points from the node recorded later to the node recorded
earlier, for every `RelationKind`. The ledger is append-only, so a reference always points
back at something that already exists; the arrows say so.

This document is the rule. The `arrow_*` tests in `src/projector/tests.rs` enforce it for
every kind on projected data, and the table below is checked against the code row by row.

## The rule

- **Stored: one semantic direction per kind.** `RelationKind::endpoints()` names one
  `(source node kind, target node kind)` pair per kind, and the source is the side making
  the claim: a decision is `BASED_ON` its evidence, evidence `SUPPORTS` a hypothesis. The
  Kuzu rel tables are bound to that pair, the Postgres projection stores
  `(relation, from, to)` exactly as projected, and every query walks it. A query never has
  to look for a relation in two places, which is what keeps staleness propagation (a
  refuted hypothesis marking every decision premised on it) complete.
- **Shown: an arrow, oriented by time.** `projector::arrow::orient` turns one stored edge
  into the arrow a reader sees. If the stored source was recorded at or after the stored
  target, the arrow runs as stored. If the stored target was recorded after the stored
  source, the arrow runs the other way and carries the relation's *reversed* label. So the
  arrow always points at the older node.
- **Time is `event_origin`.** A node's time is the ledger offset of the event that created
  it, and nothing moves it afterwards: a score, a blocker resolution, a notification
  acknowledgement or a project anchor annotates a node without changing its `event_origin`.
  A forward reference (see [Forward references](#forward-references)) is created as a
  placeholder at the referencing event; the event that really records it fills the
  placeholder in and sets the offset to that event.
- **Ties and unknowns keep the stored direction.** Nodes made by the same event are the
  same age, and a node without a recorded time cannot be ordered, so neither reverses an
  edge.
- **Actors are identities, not records.** An actor has no place on the time axis and counts
  as older than every record, so an edge to an actor always points at the actor. This is
  why 14 of the 31 kinds never reverse.
- **No inverse kinds.** The server has no "superseded by" relation. To ask what supersedes
  a decision, read `SUPERSEDES` edges *into* it; to ask what it supersedes, read them *out
  of* it. A reversed label appears only when the stored direction cannot point backward in
  time (see the table).

## What readers get

`GET /v1/graph` edges, the neighborhood edges (`hivemind query why`, MCP
`get_decision_neighborhood`, `GET /v1/decisions/why`), the CLI text summary, and the DOT
exports (CLI `dump`, MCP `dump_graph`, the TUI) all draw the same arrow:

| Field | Meaning |
|---|---|
| `from`, `to` | The arrow's ends: `from` is the newer node, `to` the older one. |
| `relation` | What the edge means, the stored kind. Unchanged whichever way the arrow runs. |
| `label` | The relation read along the arrow, an active phrase (`based on`, `informs`). |
| `reversed` | `true` when the arrow runs against the stored direction. |

A consumer that needs the side making the claim reads `from`/`to` when `reversed` is
`false` and the other way round when it is `true`. Rust readers use
`NeighborEdge::stored_source` / `stored_target`. A UI draws `from → to` with `label` and needs no
time logic of its own.

## What `GET /v1/graph` says about its nodes

The response is `{decisions, nodes, edges}`; the edges are the arrows above. Two more things
a reader needs without reconstructing them from edges:

- **`decisions[]`**: one entry per Decision node, its stored row (`id`, `title`, `rationale`,
  `topic_keys`) plus three derived fields.
  - `status`: `proposed`, `accepted`, `rejected`, `contested` or `superseded`, the word every
    other reader of a decision uses. Any incoming `SUPERSEDES` makes it `superseded`; else
    `ACCEPTED_BY` and `REJECTED_BY` together make it `contested`; else accepted only is
    `accepted`, rejected only is `rejected`, neither is `proposed`. A UI's "active" is
    `proposed` or `accepted`.
  - `deciders`: the `ACCEPTED_BY` actors as `{id, kind}` (`kind` is `human`, `agent` or
    `unknown`, from the `human:` / `agent:` id prefix), sorted by id, `[]` until someone
    accepts. Distinct from the proposer (`PROPOSED_BY`, whoever recorded it) and the
    participants (`PARTICIPATED_BY`). On a contested decision these are the acceptors; the
    rejecters are the `REJECTED_BY` edges. A decision the classifier captured from a
    transcript (`capture:<event>:<index>`) has the acceptors and rejecters the classifier named
    and no others: its `actor_id` is who proposed, made or reported it, so it is never read as
    the decider, and a capture that names no acceptor stays `proposed` until a person decides it.
    Who the conversation was held with is read from the batches the classification covers, never
    from the submission: the decision is `INITIATED_BY` the submitter of the first batch the
    classification names and `PARTICIPATED_BY` each batch's submitter and, beside a human, the
    agent tool that ran (`agent:<tool>:hook`; an agent token is already the agent). A
    classification that covers no received batch names nobody.
  - `decided_at`: the `decision.proposed` capture event's timestamp, ISO-8601 UTC (the same
    value the query layer calls `occurred_at`, e.g. `DecisionBrief`). A classified capture
    carries the time of the transcript turn it came from when it names one that has a time
    (`source_ts`, read from the received turn); otherwise the newest turn time of the batch it
    was classified from, when that batch's turns carry times (read from the ledger's received
    batch when the graph is built, so a session classified days later still reads as when it
    was said); otherwise the classified-batch event's time. `null` only for a decision from an
    event predating the ledger's timestamp backfill.
  - `slug`: the decision's stable link segment (`/decisions/<slug>`) — its title, kebab-cased
    (apostrophes dropped, so `reader's` is `readers`) and cut at the last whole word that
    fits in 60 characters (a first word longer than that is cut at 60), with a growing
    id-tail suffix (`-a1b2`, then longer) for a later decision whose title
    slugs to the same thing. Assigned once, when `decision.proposed` is first projected
    (ledger order, so replay always assigns the same slugs); nothing projected after that
    touches it, so a retitle (or any other annotation) does not move a decision's link. `null`
    only for a decision from an event predating this field.
- **`nodes[]` with `kind: "Decision"`** carry `decided_at` and `slug` the same way, so a
  whole-graph view can show when each decision was made, and link to it, without a second
  lookup into `decisions[]`.
- **`nodes[]` with `kind: "Option"`** carry `title`: the option's own label, else its id, so an
  option always has something to show. `label` stays as it was (absent when no label was ever
  recorded). A label recorded as a slug or a lettered code (`name-a-upheld`) is read as words
  (`Upheld`) when the ledger is replayed; the node then also carries `recorded_as`, the text the
  capture recorded, so a page that shows the words can show what the record says beside them. It
  is absent when the label reads as it was recorded.

Both are pure reads: three bulk edge scans (`SUPERSEDES`, `ACCEPTED_BY`, `REJECTED_BY`), no
per-decision queries and no inference.

## Kinds

Source → Target is the stored direction. The arrow label is used when the arrow runs as
stored; the reversed label when the stored target was recorded after the stored source.
`never` marks a kind whose target is an actor.

| Relation | Source → Target | Arrow label | Reversed label |
|---|---|---|---|
| `PROPOSED_BY` | Decision → Actor | proposed by | never |
| `DECISION_REQUESTED_BY` | DecisionRequest → Actor | requested by | never |
| `DECISION_REQUEST_FOR_DECISION` | DecisionRequest → Decision | asks about | answers |
| `DECISION_REQUEST_REQUIRED_OWNER` | DecisionRequest → Actor | waits on | never |
| `ACCEPTED_BY` | Decision → Actor | accepted by | never |
| `REJECTED_BY` | Decision → Actor | rejected by | never |
| `REQUEST_PROPOSED_BY` | DecisionRequest → Actor | proposed by | never |
| `REQUEST_ACCEPTED_BY` | DecisionRequest → Actor | accepted by | never |
| `REQUEST_REJECTED_BY` | DecisionRequest → Actor | rejected by | never |
| `SUPERSEDES` | Decision → Decision | supersedes | is superseded by |
| `BLOCKED_ACTOR` | Blocker → Actor | blocks | never |
| `BLOCKER_FOR_DECISION` | Blocker → Decision | waits on | unblocks |
| `BLOCKER_REQUIRED_OWNER` | Blocker → Actor | waits on | never |
| `NOTIFICATION_FOR_BLOCKER` | Notification → Blocker | is about | is announced by |
| `NOTIFICATION_RECIPIENT` | Notification → Actor | is sent to | never |
| `BASED_ON` | Decision → Evidence | based on | informs |
| `HAS_OPTION` | Decision → Option | weighs | is weighed in |
| `CHOSE` | Decision → Option | chose | is chosen in |
| `PREMISED_ON` | Option → Hypothesis | rests on | underpins |
| `PREMISED_ON_DIRECT` | Decision → Hypothesis | rests on | underpins |
| `SUPPORTS` | Evidence → Hypothesis | supports | draws on |
| `REFUTES` | Evidence → Hypothesis | refutes | is refuted by |
| `SAME_AS` | Decision → Decision | is the same as | is the same as |
| `PARTICIPATED_BY` | Decision → Actor | captured with | never |
| `INITIATED_BY` | Decision → Actor | initiated by | never |
| `PART_OF` | Project → Project | is part of | contains |
| `DEPENDS_ON` | Project → Project | depends on | is needed by |
| `FOLLOWS_FROM` | Decision → Decision | follows from | underlies |
| `ANSWERS` | Decision → Question | answers | is answered by |
| `ASK_FOR` | Ask → Question | asks | is asked by |
| `ASKED_BY` | Ask → Actor | asked by | never |

`SAME_AS` is a link between two records of one decision, never a merge: both nodes stay
as recorded. It is written by `relation.added`, and by a classified `decision` capture that
names the decision it restates (`restates_id`): the capture's node points at the restated
decision, newer to older. `recall` and the description resolvers fold linked records into
one answer (see `AGENT_FLUENT_QUERYING.md`).

Why these reverse. Most edges are written by the event that creates their source, so the
target already exists and the arrow runs as stored. The 17 kinds with a reversed label can
also be written later, by `relation.added`, `decision.superseded` or `project.linked`, or
name a placeholder that a later event fills in:

- Evidence recorded after the decision it is attached to, or a hypothesis registered after
  the evidence or option that bears on it: `BASED_ON`, `PREMISED_ON`, `PREMISED_ON_DIRECT`,
  `SUPPORTS`, `REFUTES`.
- A decision that answers a request, or unblocks a blocker, proposed after the request or
  blocker: `DECISION_REQUEST_FOR_DECISION`, `BLOCKER_FOR_DECISION`. A blocker reported
  after the notification that names it: `NOTIFICATION_FOR_BLOCKER`.
- A premise decision, a linked decision, an option, a superseded decision or a parent
  project recorded after the source: `FOLLOWS_FROM`, `SAME_AS`, `HAS_OPTION`, `CHOSE`,
  `SUPERSEDES`, `PART_OF`, `DEPENDS_ON`. A `SUPERSEDES` reverses only when the decision
  named as the superseder was recorded before the decision it supersedes.
- A question recorded after the decision that answers it: `ANSWERS`. A capture appends
  `question.recorded` after the proposal it is caused by, so the first answer to a question
  is older than its question; a later answer to the same question is newer, and its arrow
  runs as stored.
- An ask names a question: `ASK_FOR`. `hivemind ask` always ensures the `Question` node
  exists before the `Ask` node that names it (in the same call when the question is new), so
  in practice this arrow never reverses; it carries a real reversed label anyway because its
  target is not an `Actor`.

A `Notification` node is created by `notification.sent` (about a blocker, with a
`NOTIFICATION_FOR_BLOCKER` edge) or by `suggestion.surfaced` (about a finding: it carries
`finding_id` and `decision_id` and has only the `NOTIFICATION_RECIPIENT` edge, never a
blocker edge). `notification.acknowledged` annotates either with `ack_at`, `snooze_until`
and `action`.

## Forward references

`decision.requested`, `blocker.reported`, `notification.sent` and classified captures
may name a `Decision`, `Blocker`, `Hypothesis` or `Evidence` id that no event has
recorded yet. The projector creates a placeholder node for it at that event, so the edge
never dangles. The event that really records the node fills the placeholder in later and
becomes the node's `event_origin`, so an arrow that first ran as stored turns around once
the node it names is really recorded.

## Changing a direction or a label

The stored direction of a kind, and the two labels it carries, are part of the wire
contract: the direction changes the Kuzu rel tables, the projected Postgres rows and every
query, and a label is what a UI shows. Either needs the owner's sign-off recorded on the
tracking bead before the code lands. Any change here also has to keep
`RelationKind::arrow_labels` and this table in step; the doc test fails otherwise.
