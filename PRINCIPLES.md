# HiveMind Principles

This document names the constraints HiveMind cannot trade away. They are
load-bearing: every architectural decision, every feature, every refactor must
respect them. Consult this doc when a proposed change feels clever but
suspicious — that feeling usually means a principle is at risk.

The vision tells you why HiveMind exists. The principles tell you what HiveMind
will and will not become along the way.

---

## 1. The ledger is unconditional

Events are appended. State is derived. No smart behavior — no search, ranking,
summarization, model call, similarity, or recommendation — ever touches the
write path. The ledger is the part of HiveMind that has to stay trustworthy at
all costs, and trustworthiness comes from refusing to put anything
probabilistic between the actor and the record.

If a feature requires reaching from the agentic layer back into ingest or
projection, the feature is the wrong shape, not the architecture.

## 2. Every state change is auditable

There is always a complete, replayable record of how HiveMind reached its
current state. Status, derivations, reversals, supersessions — all can be
reconstructed from the audit record without inference or guesswork. State
you cannot replay is state you cannot audit, and HiveMind exists to be
audited.

This is the load-bearing property. *How* it is realized — event-sourcing
today, possibly a different model tomorrow — is an architectural decision
documented in `docs/ARCHITECTURE.md`. The principle is the property, not the
mechanism. Any implementation that preserves complete auditability satisfies
the principle; any implementation that doesn't, doesn't, no matter how
elegant.

This is what makes a decision survivable across migrations, backends, and
years.

## 3. Humans, agents, and systems are peers as actors

Every event carries an actor. The actor is named, never anonymous. The actor's
*kind* — human, agent, system — never grants or revokes privilege in the
write or query paths. An agent's decision and a human's decision have the same
shape, the same review surface, and the same standing.

This is the principle that makes governance possible: humans can review agent
decisions because the agent's decisions are first-class, not second-class.

## 4. Disagreement is information, not error

When two actors take incompatible positions on a decision, HiveMind records
both and surfaces the `contested` status. Disagreement is never resolved by
overwriting, deduplication, silent merge, or last-write-wins. The conflict is
the signal.

A system that hides disagreement to look tidy is lying about the organization
it serves.

## 5. Supersession is explicit

When a decision is replaced, the old decision is not deleted, mutated, or
hidden. A new decision is created with a typed `SUPERSEDES` edge. The
history of "we changed our mind, and here is why" stays readable forever.

You should be able to follow any decision backward through every reversal to
its original proposal.

## 6. Provenance is mandatory

Every write carries actor, source (`human` or `agent`), and session. Anonymous
writes are rejected. The cost of a few mandatory fields per event is
negligible compared to the cost of an unattributable claim showing up later
in an audit, a postmortem, or a dispute.

## 7. The boundary between layers is enforced, not aspirational

The three-layer architecture — write, query, agentic — is not a diagram in
a doc. It is a property of the codebase. Layer 1 (write) does not import from
layer 3 (agentic). Layer 2 (query) does not call LLMs, score, cluster, or rank.
If a change requires crossing these boundaries, the change is wrong, even if
it is convenient.

This is the principle most likely to be tested when HiveMind starts to scale.
Smart behavior wants to creep down. The principle is what keeps it where it
belongs.

---

## How to use this document

When a proposal lands that seems to violate one of these principles, the
proposal does not get to argue its way past the principle. It gets to:

1. Find a different design that respects the principle, or
2. Make an explicit, named case for why this principle should change — which
   means rewriting this doc, in the open, with reasoning.

Principles change. They do not erode.
