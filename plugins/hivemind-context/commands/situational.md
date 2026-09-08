---
name: situational
description: What should I know before I touch this? Decisions bearing on the current diff/paths/branch, no question needed.
argument-hint: '[--paths a,b] [--diff] [--branch] [--cwd] [--since-branch-point] [--summary]'
allowed-tools: Bash(${CLAUDE_PLUGIN_ROOT}/scripts/situational.sh:*)
disable-model-invocation: true
---

Surface the decisions that bear on the current working situation — no
question, no id. Defaults to the current git diff / staged set when no
`--paths`/`--diff`/`--branch`/`--cwd` is given. Runs `query situational`:

`$ARGUMENTS`

Run the plugin helper:

```bash
${CLAUDE_PLUGIN_ROOT}/scripts/situational.sh $ARGUMENTS
```

Return the CLI result directly and preserve `truncated`, `matched_via`
(why each decision surfaced), `still_holds`, and `changed_since` fields —
do not summarize away staleness or the match reason.
