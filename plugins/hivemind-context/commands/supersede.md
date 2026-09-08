---
name: supersede
description: Replace a decision with a new one, resolved by description, never by id.
argument-hint: '"description of the old decision" --title "..." --rationale "..."'
allowed-tools: Bash(${CLAUDE_PLUGIN_ROOT}/scripts/supersede.sh:*)
disable-model-invocation: true
---

Replace an existing decision with a new one, resolved by description. This
is a WRITE verb — the ambiguity gate is strict. Runs `supersede`:

`$ARGUMENTS`

Run the plugin helper:

```bash
${CLAUDE_PLUGIN_ROOT}/scripts/supersede.sh $ARGUMENTS
```

If the description does not resolve to exactly one decision, the CLI
returns the candidate list and performs **no write**. Show the candidates
and re-invoke with `--pick N` (or `--old <id>`) once you're certain — never
guess which decision to supersede.
