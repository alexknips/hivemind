---
name: query-decisions
description: Query HiveMind decisions from the configured ledger
argument-hint: '[--q "..."] [--actor-id actor] [--source agent|human] [--limit 10]'
allowed-tools: Bash(${CLAUDE_PLUGIN_ROOT}/scripts/query-decisions.sh:*)
disable-model-invocation: true
---

Query HiveMind decisions from the configured ledger with the supplied
arguments. The helper runs `query search_decisions`:

`$ARGUMENTS`

Run the plugin helper:

```bash
${CLAUDE_PLUGIN_ROOT}/scripts/query-decisions.sh $ARGUMENTS
```

If no actor is supplied, default to the current Claude Code session actor:
`agent:claude:<session>`. Return the CLI result directly and preserve
`truncated` fields, statuses, contested decisions, and stale dependencies.
