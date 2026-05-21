---
name: capture-decision
description: Capture one HiveMind decision in the configured ledger
argument-hint: '--title "..." --rationale "..." --topic-keys topic[,topic] --options option[,option] [--chose option] [--source agent|human]'
allowed-tools: Bash(${CLAUDE_PLUGIN_ROOT}/scripts/capture-decision.sh:*)
disable-model-invocation: true
---

Capture exactly one HiveMind decision from the supplied arguments:

`$ARGUMENTS`

Run the plugin helper:

```bash
${CLAUDE_PLUGIN_ROOT}/scripts/capture-decision.sh $ARGUMENTS
```

The helper prints a confirmation and a follow-up
`/hivemind-capture:query-decisions` command scoped to the recorded actor.

Default to `--source agent`, which records
`actor_id=agent:claude:<session>` and `source=agent`. Use `--source human`
only when the user explicitly asks you to record their decision as a human
write.

Do not query, rank, summarize, or infer related decisions before capturing.
This command is a write-layer path only.
