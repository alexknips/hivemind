# HiveMind Vision

## The bet

The next decade of knowledge work will be done by humans and AI agents together.
The open question is whether humans stay in the loop or hand decision-making to
AI wholesale. We bet on the former — and we believe that requires a substrate
where every decision, by every actor, is recorded, queryable, and contestable.

HiveMind is that substrate.

## The pain

Today, decisions live in chat threads, Slack history, scattered documents,
agent transcripts, and the heads of the people who were in the meeting. A senior engineer reviewing a
junior's work gets sent a Claude chat URL and has to read it to understand the
reasoning. An agent making a follow-up choice has no structured place to learn
what the previous agent decided or why. Disagreement is invisible until it
resurfaces as conflict months later. Compliance teams can't reconstruct who
chose what.

Wikis and documents do not solve this. They capture artifacts, not choices.
They have no actor model. They can't represent "this superseded that on Tuesday
because new evidence came in."

## What HiveMind is

HiveMind is an event-sourced, graph-projected decision ledger. Every entry
records:

- the decision that was made
- the actor who made it — human, agent, or system, treated as peers
- the options considered
- the evidence it rested on
- the hypotheses still in flight
- the disagreement, where it existed
- the supersession, when it happened

Status (`proposed`, `accepted`, `contested`, `superseded`) is derived from the
graph, not stored as a property. The ledger never lies because the smart
behavior — search, ranking, compactification, recommendations — cannot reach
into the write path. Smart layers run on top of the ledger, never inside it.

## Who it's for

Anyone whose work produces decisions worth remembering, and whose work
increasingly includes agents acting on their behalf.

- **Small teams** — keep decision history alive past the conversation that
  created it.
- **Large organizations** — cross-team decision visibility without crawling a
  wiki.
- **Technical leaders and their juniors** — review reasoning as structured
  decisions, not as shared chat URLs.
- **Operators of agents** — see what your agents decided the same way you see
  your own choices, and disagree with them when warranted.
- **Compliance, audit, legal** — a reviewable trail of who chose what, when, and
  on what evidence.
- **Beyond engineering** — product, research, operations, policy. Anywhere
  knowledge work creates choices.

The defining user is the operator of an agent who refuses to abdicate the
decision to the agent.

## What changes when HiveMind exists

- Juniors don't share chat URLs. They share decisions.
- One agent's choice becomes the next agent's structured starting context,
  instead of being regenerated from scratch.
- Disagreement is named, dated, and reviewable — not forgotten until it
  resurfaces.
- "We don't remember why we decided that" stops being a normal answer.
- Humans retain governance over agentic work because they can see, query, and
  contest every agent decision after the fact — at the speed of a query, not
  the speed of reading a transcript.

The alternative future is one where the only way to find out why something was
decided is to ask an AI what it remembers. We are building the alternative to
that future.

## What this depends on

HiveMind only works if two capabilities are excellent: **search** (find the
decisions that matter, given how you remember them) and **compactification**
(keep the ledger queryable as it grows from hundreds to millions of decisions).
Both live in the agentic layer that sits above the ledger. The vision rests on
getting them right.

## What HiveMind is not

- Not a chat archive — chat is conversation; HiveMind is conclusion.
- Not a notes app — notes are personal; HiveMind is organizational.
- Not a task tracker — tasks are work to do; HiveMind is what was decided.
- Not agent memory — agent memory serves the agent's next prompt; HiveMind
  serves the human who needs to govern the agent.
- Not a wiki replacement — wikis hold artifacts; HiveMind holds the structure
  of how those artifacts came to be.
