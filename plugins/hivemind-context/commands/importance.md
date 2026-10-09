---
name: importance
description: Which decisions carry the most impact, and who decided them (a person or an agent).
argument-hint: '[--limit 10] [--cursor C] [--include-not-in-force] [--summary]'
allowed-tools: Bash(${CLAUDE_PLUGIN_ROOT}/scripts/importance.sh:*)
disable-model-invocation: true
---

List the decisions ordered by impact, most first, each with who decided it.
Runs `query rank_decisions_by_importance`:

`$ARGUMENTS`

Run the plugin helper:

```bash
${CLAUDE_PLUGIN_ROOT}/scripts/importance.sh $ARGUMENTS
```

A decision is ranked when other decisions follow from it. The rest follow
unranked and read `not_assessed`: nothing recorded says they matter, which is
not the same as saying they do not. Each row names who decided it
(`decided_by.kind`: person, agent, agent_within_delegation, mixed, unknown or
none_recorded); the actor who only recorded a decision is never shown as its
decider. Report `not_assessed` and `none_recorded` as they are — never turn
them into a guess.
