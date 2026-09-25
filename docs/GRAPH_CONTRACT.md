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
  why 13 of the 28 kinds never reverse.
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
  `topic_keys`) plus two derived fields.
  - `status`: `proposed`, `accepted`, `rejected`, `contested` or `superseded`, the word every
    other reader of a decision uses. Any incoming `SUPERSEDES` makes it `superseded`; else
    `ACCEPTED_BY` and `REJECTED_BY` together make it `contested`; else accepted only is
    `accepted`, rejected only is `rejected`, neither is `proposed`. A UI's "active" is
    `proposed` or `accepted`.
  - `deciders`: the `ACCEPTED_BY` actors as `{id, kind}` (`kind` is `human`, `agent` or
    `unknown`, from the `human:` / `agent:` id prefix), sorted by id, `[]` until someone
    accepts. Distinct from the proposer (`PROPOSED_BY`, whoever recorded it) and the
    participants (`PARTICIPATED_BY`). On a contested decision these are the acceptors; the
    rejecters are the `REJECTED_BY` edges.
- **`nodes[]` with `kind: "Option"`** carry `title`: the option's own label, else its id, so an
  option always has something to show. `label` stays as it was (absent when no label was ever
  recorded).

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

Why these reverse. Most edges are written by the event that creates their source, so the
target already exists and the arrow runs as stored. The 15 kinds with a reversed label can
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
