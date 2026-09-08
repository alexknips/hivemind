---
name: disagree
description: Push back on a decision by description, never by id. Records dissent; never silently overrides.
argument-hint: '"description of the decision" --reason "why you disagree"'
allowed-tools: Bash(${CLAUDE_PLUGIN_ROOT}/scripts/disagree.sh:*)
disable-model-invocation: true
---

Record disagreement with a decision, resolved by description. This is a
WRITE verb — the ambiguity gate is strict. Runs `disagree`:

`$ARGUMENTS`

Run the plugin helper:

```bash
${CLAUDE_PLUGIN_ROOT}/scripts/disagree.sh $ARGUMENTS
```

If the description does not resolve to exactly one decision, the CLI
returns the candidate list and performs **no write**. Show the candidates
and re-invoke with `--pick N` (or `--decision <id>`) once you're certain —
never guess which decision to contest.
