---
allowed-tools: Bash(.claude/scripts/capture-decision.sh:*)
argument-hint: --title "..." --rationale "..." --topic-keys topic[,topic] --options option[,option] [--chose option] [--source human|agent]
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
decision; that records `actor_id=agent:claude:<session>` and `source=agent`.

Do not query, rank, summarize, or infer related decisions. This command is a
write-layer capture path only.
