---
name: recall
description: What did we decide about X? Free-text search + summary in one call, never an id.
argument-hint: '"what did we decide about X" [--topic t,...] [--source agent|human] [--summary]'
allowed-tools: Bash(${CLAUDE_PLUGIN_ROOT}/scripts/recall.sh:*)
disable-model-invocation: true
---

Answer "what did we decide about X?" from free text. Runs `query recall`:

`$ARGUMENTS`

Run the plugin helper:

```bash
${CLAUDE_PLUGIN_ROOT}/scripts/recall.sh $ARGUMENTS
```

Return the CLI result directly and preserve `truncated` fields, statuses,
contested decisions, and stale dependencies.
