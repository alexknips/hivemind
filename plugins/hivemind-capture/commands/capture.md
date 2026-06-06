---
name: capture
description: Capture one HiveMind decision-memory item in the configured ledger
argument-hint: '"<text>" [--kind decision|evidence|hypothesis|blocker|decision-request|notification] [decision flags] [--source agent|human]'
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
/hivemind-capture:capture "selected direction" --kind decision --title "..." --rationale "..." --topic-keys topic[,topic] --options option[,option] [--chose option]
```

When `--kind` is omitted, the helper delegates classification to the configured
`hivemind-classifier` subagent if it is installed; otherwise it emits nothing.
The helper recognizes blocker, decision-request, and notification as schema
kinds, but returns a clear unsupported-kind error until the command layer has
canonical capture paths for those event shapes.

Default to `--source agent`, which records
`actor_id=agent:claude:<session>` and `source=agent`. Use `--source human`
only when the user explicitly asks you to record their write as human-authored.

Do not query, rank, summarize, or infer related decisions before capturing.
This command is a write-layer path only.
