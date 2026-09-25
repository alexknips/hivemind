---
name: capture-decision
description: Capture one HiveMind decision in the configured ledger using the legacy kind-locked path
argument-hint: '--title "..." --rationale "..." --topic-keys topic[,topic] --options option[,option] [--chose option] [--decided-by actor-id] [--delegated-by human:name] [--still-proposed] [--project handle] [--source agent|human]'
allowed-tools: Bash(${CLAUDE_PLUGIN_ROOT}/scripts/capture-decision.sh:*)
disable-model-invocation: true
---

`/hivemind-capture:capture` is the primary capture command. This legacy command
keeps existing decision-only usages working and always dispatches with
`--kind decision`.

Capture exactly one HiveMind decision from the supplied arguments:

`$ARGUMENTS`

Run the plugin helper:

```bash
${CLAUDE_PLUGIN_ROOT}/scripts/capture-decision.sh $ARGUMENTS
```

The helper prints a confirmation and a follow-up
`/hivemind-capture:query-decisions` command scoped to the recorded actor.

Default to `--source agent`, which records `actor_id=agent:claude:<name>`
(a stable identity, preferring Gas City's `GC_AGENT`/`GC_ALIAS` over a raw
session id) and `source=agent`. Use `--source human` only when the user
explicitly asks you to record their decision as a human write.

`--chose option` means the decision was already made: it self-accepts
immediately (or accepts from `--decided-by <actor-id>` when someone else
decided). Pass `--still-proposed` instead to float a leaning that still
awaits someone else's decision.

When you decided within a scope a human explicitly delegated to you, add
`--delegated-by <human:name>` so the record shows the delegation rather than an
agent deciding alone. It requires `--chose` and an agent actor.

The confirmation also names the decision's project (`project: billing
(folder_marker)`), worked out from the working folder; pass `--project <handle>`
only when you were told which project. When it says the decision was saved to
the personal project and the folder is not attached to a project yet, tell the
user, with the attach hint it prints.

Do not query, rank, summarize, or infer related decisions before capturing.
This command is a write-layer path only.
