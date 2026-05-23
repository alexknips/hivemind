# HiveMind Strategy

This document does not plan work. Beads plan work.

This document names the **directions** that count as progress toward the vision,
the directions that explicitly do not, and the standards by which any bead can
be judged. Read it to ask one question:

> Does this bead advance one of these fronts, respect the principles, and serve
> the vision?

If yes, the bead is well-aimed. If no, the bead is wrong — or this document is
wrong, and it needs to be rewritten in the open before the bead proceeds.

---

## Active fronts

These are the directions HiveMind is investing in right now. Beads that
advance any of these fronts are well-aimed by default. Multiple fronts proceed
in parallel; nothing here is sequenced.

### Capture quality

Make decision capture so cheap that no human and no agent ever skips it. Every
friction point — extra arguments, unclear defaults, surprising failures,
verbose output, ambiguous provenance — is a target.

A bead advances this front if it removes a reason someone would *not* capture a
decision.

### Search

Make finding a decision faster than re-deciding it. Topic, status, actor,
time, content, supersession — all should be reachable through one or a few
intent-shaped queries, not by reading the ledger.

A bead advances this front if it lets a user find a decision they remember
without already knowing its ID.

### Governance UX

Make the governance moments visible: disagreeing with an agent, superseding
an earlier decision, surfacing the `contested` status, walking a decision's
history. The vision rests on humans staying in the loop; this front is what
makes that loop usable.

A bead advances this front if it shortens the path from "I want to push back
on a decision" to "I have pushed back, and the push-back is in the ledger."

### Shared backend

Move HiveMind from local-prototype to multi-machine, multi-actor, multi-org.
The ledger should be a real service when more than one human or agent needs
to read and write the same decision history.

A bead advances this front if it makes a shared deployment more real, more
testable, or more deployable.

### Ecosystem reach

Extend HiveMind to where decisions are already being made: more MCP clients,
more agent harnesses, more capture entry points (slash commands, hooks,
plugins, document import). The vision applies to all knowledge work, so the
capture surfaces should too.

A bead advances this front if it brings HiveMind to a new decision surface
without compromising the principles.

### Layer-3 capabilities

Compactification, similarity, ranking, recommendations, summarization. The
capabilities the vision depends on at scale. These belong **above** the
ledger, never inside it (see [PRINCIPLES.md](PRINCIPLES.md#1-the-ledger-is-unconditional)).

A bead advances this front if it builds layer-3 behavior that respects the
boundary. A bead that proposes layer-3 logic embedded in the write or query
path does not advance this front. It violates the architecture.

---

## Not active fronts

These are directions HiveMind is **not** investing in. Beads aimed here will
be redirected or rejected unless this document changes first.

- **Becoming a chat archive, notes app, task tracker, or wiki replacement.**
  See VISION.md — these are scope refusals.
- **Vendor-specific integrations that lock the data behind one tool.** The
  ledger must remain portable.
- **Recommendation or ranking surfaces that influence the write path.**
  Forbidden by [PRINCIPLES.md §1](PRINCIPLES.md#1-the-ledger-is-unconditional).
- **"Smart" status determination** (an LLM deciding whether a decision is
  accepted). Forbidden by [PRINCIPLES.md §2](PRINCIPLES.md#2-events-are-authoritative-state-is-derived).
- **Optimizing for a single deployment scale** before the dogfood loop
  proves it. Premature scale work tends to harden the wrong shape.

---

## How to use this document

When creating, claiming, or reviewing a bead, ask:

1. **Does it advance one of the active fronts?** If no, it likely shouldn't
   exist as a bead right now — or this document is out of date.
2. **Does it respect the principles?** If no, it is the wrong shape. Redesign
   or refuse it.
3. **Does it serve the vision, or sit adjacent to it?** Adjacent work is not
   wrong, but it should be deprioritized until the active fronts have caught
   up.

Strategy changes when the world changes. The fronts here are not eternal.
They are what HiveMind is investing in *now*.

How fast HiveMind moves depends on how many agents are pulling beads in
parallel and how complex those beads are — not on a calendar. A bead is sized
by its complexity, not by a date. A front advances when its beads close, at
whatever rate parallel agent throughput makes possible.
