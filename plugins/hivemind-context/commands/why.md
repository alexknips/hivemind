---
name: why
description: Why was this decided? Rationale and neighborhood for a decision, resolved by description.
argument-hint: '"description of the decision" [--pick N] [--depth 1] [--summary]'
allowed-tools: Bash(${CLAUDE_PLUGIN_ROOT}/scripts/why.sh:*)
disable-model-invocation: true
---

Resolve a decision by description (never guess an id) and show its
rationale and neighborhood. Runs `query why` (alias of
`get_decision_neighborhood`):

`$ARGUMENTS`

Run the plugin helper:

```bash
${CLAUDE_PLUGIN_ROOT}/scripts/why.sh $ARGUMENTS
```

If the description resolves ambiguously, the CLI returns a numbered
candidate list instead of guessing. Show the candidates to the user/agent
and re-invoke with `--pick N` or a bare `#N` once the right one is clear —
never pick silently.
