---
name: supersede
description: Replace a decision with a new one, resolved by description, never by id.
argument-hint: '"description of the old decision" --title "..." --rationale "..." --rests-on-decision "..." | --rests-on-evidence "..." --evidence-source "..." | --rests-on-assumption "..." | --bet'
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

The replacement must say what it rests on: one of `--rests-on-decision "..."`
(a decision it follows from), `--rests-on-evidence "..."` with
`--evidence-source "..."` (something observed, and where),
`--rests-on-assumption "..."` (something assumed), or `--bet` (a declared bet).
A supersede that names none of these is refused ("a captured decision must say
what it rests on") and nothing is written.

`--chose <option>` means the replacement was already decided: it is accepted
from your actor right away. Add `--still-proposed` to keep an open
recommendation at `proposed`. The confirmation shows `new_status=`.

The replacement's project is worked out from the working folder (or named with
`--project <handle>`); with none found it inherits the project the old decision
is filed in now (after a move, the project it was moved to). A decision filed
in a personal project is replaced into your own personal project instead —
personal projects belong to one actor — and the confirmation says so.
The confirmation shows `project=` and `project_source=` (`inherited` for the
first case, `personal_fallback` for the personal-project exception).

If the description does not resolve to exactly one decision, the CLI
returns the candidate list and performs **no write**. Show the candidates
and re-invoke with `--pick N` (or `--old <id>`) once you're certain — never
guess which decision to supersede.
