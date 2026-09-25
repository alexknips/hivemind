---
name: capture
description: Capture one HiveMind decision-memory item in the configured ledger
argument-hint: '"<text>" [--kind decision|evidence|hypothesis|blocker|decision-request|notification] [decision flags, incl. what it rests on] [--source agent|human]'
allowed-tools: Bash(${CLAUDE_PLUGIN_ROOT}/scripts/capture.sh:*)
disable-model-invocation: true
---

Capture exactly one HiveMind decision-memory item from the supplied arguments:

`$ARGUMENTS`

Run the plugin helper:

```bash
${CLAUDE_PLUGIN_ROOT}/scripts/capture.sh $ARGUMENTS
```

Use `/hivemind-capture:capture "<observation>" --kind evidence` for durable
observations and `/hivemind-capture:capture "<claim>" --kind hypothesis` for
assumptions that may later be supported or refuted.

For decisions, keep using structured decision fields:

```text
/hivemind-capture:capture "selected direction" --kind decision --title "..." --rationale "..." --topic-keys topic[,topic] --options option[,option] [--chose option] [--decided-by actor-id] [--delegated-by human:name] (--rests-on-decision "..." | --rests-on-evidence "..." --evidence-source "..." | --rests-on-assumption "..." | --bet ["..."])
```

Every decision must say what it rests on. Before writing, answer "what does
this rest on?" with one or more of:

- a decision we already made: `--rests-on-decision "<described the way you
  would describe it>"`, or `'#N'` / the `decision-...` id from the consult you
  made before acting;
- something observed: `--rests-on-evidence "<what>" --evidence-source "<where:
  URL, file@commit, test run, measurement>"`;
- something we assume: `--rests-on-assumption "<statement>"`;
- nothing yet: `--bet ["<statement>"]`, optionally `--would-change-if "<what
  would change our mind>"` and `--check-by YYYY-MM-DD`.

An answer that fits none of the four is recorded as an assumption, as given.
The usual loop is consult (`/hivemind-context:situational` or `recall`), decide,
then capture with the consulted decision as the premise.

The decider's own words are not a grounding: never pass a Slack message or chat
reply as `--rests-on-evidence`. They go in `--quote` (verbatim), paired with
`--question` (what they answered). Pass `--confidence low|medium|high` only when
the decider's own words state it; omit it otherwise.

A capture that names nothing exits 2 and writes nothing. After any refusal (no
grounding, an ambiguous premise, no match), add the grounding and re-run: never
drop the capture. For an ambiguous premise, re-run with `--rests-on-decision
'#N'` in place of the description.

`--chose option` means the decision was already made: it self-accepts
immediately (or accepts from `--decided-by <actor-id>` when someone else
decided). Pass `--still-proposed` instead to float a leaning that still
awaits someone else's decision.

When you decided within a scope a human explicitly delegated to you, add
`--delegated-by <human:name>` so the record shows the delegation rather than an
agent deciding alone. It requires `--chose` and an agent actor.

When `--kind` is omitted, the helper delegates classification to the configured
`hivemind-classifier` subagent if it is installed; otherwise it emits nothing.
The helper recognizes blocker, decision-request, and notification as schema
kinds, but returns a clear unsupported-kind error until the command layer has
canonical capture paths for those event shapes.

Default to `--source agent`, which records `actor_id=agent:claude:<name>`
(a stable identity, preferring Gas City's `GC_AGENT`/`GC_ALIAS` over a raw
session id) and `source=agent`. Use `--source human` only when the user
explicitly asks you to record their write as human-authored.

A decision's confirmation also names its project (`project: billing
(folder_marker)`): the helper has the CLI work it out from the working folder,
or pass `--project <handle>` only when you were told which project. When the
confirmation says the decision was saved to the personal project and the folder
is not attached to a project yet, tell the user, with the attach hint it prints
(the hint names `hivemind project anchor`; a folder is attached by committing a
`.hivemind-project` file holding the project handle).

This command is a write-layer path only: it does not query, rank, summarize, or
infer related decisions itself. Consulting is a separate step you take before
deciding, and it never decides whether to capture.
