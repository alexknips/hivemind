---
name: verify
description: Did that decision hold up? Outcome check for a decision, resolved by description.
argument-hint: '"description of the decision" [--pick N] [--summary]'
allowed-tools: Bash(${CLAUDE_PLUGIN_ROOT}/scripts/verify.sh:*)
disable-model-invocation: true
---

Resolve a decision by description (never guess an id) and check whether it
still holds. Runs `query verify` (alias of `get_decision_outcome`):

`$ARGUMENTS`

Run the plugin helper:

```bash
${CLAUDE_PLUGIN_ROOT}/scripts/verify.sh $ARGUMENTS
```

Leads with the decision, rationale, rejected options, who decided, and
whether it still holds (`held_up` + structured reasons — superseded, stale
premises, contested, thin structure). If the description resolves
ambiguously, the CLI returns a numbered candidate list instead of guessing
— re-invoke with `--pick N` or `#N`.
