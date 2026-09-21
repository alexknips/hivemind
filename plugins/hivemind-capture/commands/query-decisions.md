---
name: query-decisions
description: What did we decide about X? Free-text search over the configured HiveMind ledger.
argument-hint: '["free text"] [--q "..."] [--actor-id actor] [--source agent|human] [--limit 10]'
allowed-tools: Bash(${CLAUDE_PLUGIN_ROOT}/scripts/query-decisions.sh:*)
disable-model-invocation: true
---

Query HiveMind decisions from the configured ledger with the supplied
arguments. The helper forwards to the fluent verb `query recall` — free text
first, never a decision id:

`$ARGUMENTS`

Run the plugin helper. Quote `$ARGUMENTS` — free text is normally several
words and an unquoted expansion splits it into separate shell arguments,
which the CLI's positional query field rejects:

```bash
${CLAUDE_PLUGIN_ROOT}/scripts/query-decisions.sh "$ARGUMENTS"
```

No actor or source filter is applied unless the caller passes --actor-id
or --source: this searches every decision in the ledger, not just the
ones captured by the current session. Return the CLI result directly and
preserve `truncated` fields, statuses, contested decisions, and stale
dependencies.

For follow-up on one specific decision — its rationale and neighborhood, or
whether it still holds — or to contest/supersede it, install the
`hivemind-context` plugin instead. Its verbs (`recall`/`why`/`verify`/
`chain`/`disagree`/`supersede`) resolve by description too, and cover the
single-decision follow-up this command does not.
