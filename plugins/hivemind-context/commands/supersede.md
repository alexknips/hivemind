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

`--chose <option>` means the replacement was already decided: it is accepted
from your actor right away. Add `--still-proposed` to keep an open
recommendation at `proposed`. The confirmation shows `new_status=`.

The replacement's project is worked out from the working folder (or named with
`--project <handle>`); with none found it stays in the old decision's project.
The confirmation shows `project=` and `project_source=`.

If the description does not resolve to exactly one decision, the CLI
returns the candidate list and performs **no write**. Show the candidates
and re-invoke with `--pick N` (or `--old <id>`) once you're certain — never
guess which decision to supersede.
