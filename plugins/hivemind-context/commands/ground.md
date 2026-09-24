---
name: ground
description: Say what an existing decision rests on, after the fact, resolved by description, never by id.
argument-hint: '"description of the decision" --rests-on-decision "..." | --rests-on-evidence "..." --evidence-source "..." | --rests-on-assumption "..." | --bet'
allowed-tools: Bash(${CLAUDE_PLUGIN_ROOT}/scripts/ground.sh:*)
disable-model-invocation: true
---

Add what an existing decision rests on — a decision it follows from,
something observed and where, something assumed, or a declared bet. This is
a WRITE verb — the ambiguity gate is strict. Runs `ground`:

`$ARGUMENTS`

Run the plugin helper:

```bash
${CLAUDE_PLUGIN_ROOT}/scripts/ground.sh $ARGUMENTS
```

The grounding is append-only and attributed to you, so a reader sees it was
added later rather than at capture. If the description does not resolve to
exactly one decision — or a `--rests-on-decision` premise matches more than
one — the CLI returns the candidate list and performs **no write**. Show the
candidates and re-invoke with `--pick N` (or `--id <id>`, or `'#N'` for a
premise) once you're certain — never guess which decision to ground. A
premise that already rests on this decision is refused: it would close a loop.
