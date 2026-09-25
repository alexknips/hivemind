---
allowed-tools: Bash(.claude/scripts/capture-decision.sh:*)
argument-hint: --title "..." --rationale "..." --topic-keys topic[,topic] --options option[,option] [--chose option] [--decided-by actor-id] [--still-proposed] (--rests-on-decision "..." | --rests-on-evidence "..." --evidence-source "..." | --rests-on-assumption "..." | --bet ["..."]) [--source human|agent]
description: Capture a HiveMind decision in the local ledger
---

When `/capture-decision` is invoked, capture exactly one HiveMind decision from
the supplied arguments:

`$ARGUMENTS`

Use the repository-local capture helper:

```bash
.claude/scripts/capture-decision.sh $ARGUMENTS
```

Default to `--source human`, which records `actor_id=human:<git-user>` and
`source=human` for a decision explicitly requested through this slash command.
Use `--source agent` only when you are recording an autonomous Claude Code
decision; that records `actor_id=agent:claude:<name>` (a stable identity,
preferring Gas City's `GC_AGENT`/`GC_ALIAS` over a raw session id) and
`source=agent`.

`--chose option` means the decision was already made: it self-accepts
immediately (or accepts from `--decided-by <actor-id>` when someone else,
such as the human who asked for this capture, actually decided). Pass
`--still-proposed` instead to float a leaning that still awaits someone
else's decision.

Every decision must say what it rests on, so answer "what does this rest on?"
before writing, with one or more of:

- `--rests-on-decision "<description>"`: a decision we already made, described
  the way you would describe it (or `'#N'` / the id from a consult);
- `--rests-on-evidence "<what was observed>" --evidence-source "<where>"`:
  something observed, with a URL, `file@commit`, test run or measurement;
- `--rests-on-assumption "<statement>"`: something we assume;
- `--bet ["<statement>"]`: nothing yet, a declared judgement call, optionally
  with `--would-change-if "<text>"` and `--check-by YYYY-MM-DD`.

A capture that names none exits 2 and writes nothing: add the grounding and
re-run, never drop the capture. The decider's own words are not a grounding; a
Slack message or chat reply goes in `--quote` with `--question`, never in
`--rests-on-evidence`. Pass `--confidence low|medium|high` only when the
decider's own words state it.

This command is a write-layer capture path only: it does not query, rank,
summarize, or infer related decisions itself.
