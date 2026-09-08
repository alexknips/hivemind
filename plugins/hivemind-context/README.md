# HiveMind Context Plugin

Status: **name provisional.** Working name `hivemind-context`; see
[Naming](#naming) below — the mayor and Alex may rename this directory,
`plugin.json` `name` fields, and the marketplace entries before this ships.

This directory is both the Codex context plugin and the Claude Code
`hivemind-context` plugin. It is the query/contest counterpart to
`plugins/hivemind-capture`: capture writes decisions, this plugin reads and
contests them — fluently, by description, never by id. It ships **no MCP
server**; every command is a thin `scripts/*.sh` wrapper that execs the
`hivemind` binary directly. See
[docs/AGENT_DECISION_CONTEXT.md](../../docs/AGENT_DECISION_CONTEXT.md) for
why this plugin exists alongside the MCP surface rather than instead of it.

The Claude package installs:

- `/hivemind-context:situational` — "what should I know before I touch
  this?" from the current diff/paths/branch, no question needed.
- `/hivemind-context:recall` — "what did we decide about X?", free text.
- `/hivemind-context:why` — rationale and neighborhood for a decision,
  resolved by description.
- `/hivemind-context:verify` — "did that hold up?", resolved by
  description.
- `/hivemind-context:disagree` — push back on a decision by description
  (write verb, strict ambiguity gate).
- `/hivemind-context:supersede` — replace a decision by description (write
  verb, strict ambiguity gate).
- The `hivemind-context` skill, which teaches when to consult HiveMind
  before changing code, and forbids inventing decision ids.

Every command maps 1:1 to one CLI verb from `hivemind-tenv.1`/`tenv.2`'s
fluent surface (`query situational`, `query recall`, `query why`,
`query verify`, `disagree`, `supersede`). None of them take a `decision_id`
as primary input — free text (or `#N` from a previous ambiguous result) is
the interface; `--id`/`--decision`/`--old`/`--pick N` remain as escape
hatches for callers that already have one.

## Install

Install the HiveMind CLI first so `hivemind` is on `PATH`.

From Claude Code, add this repository as a marketplace and install the
plugin:

```text
/plugin marketplace add alexknips/hivemind
/plugin install hivemind-context@hivemind
/reload-plugins
```

For local development from this checkout:

```text
/plugin marketplace add .
/plugin install hivemind-context@hivemind
/reload-plugins
```

Claude Code can also test the plugin directory directly:

```bash
claude --plugin-dir ./plugins/hivemind-context
```

The default ledger is the project-local `./hivemind/` directory. To use
another ledger, set the plugin option `hivemind_dir`, export `HIVEMIND_DIR`,
or pass `--hivemind-dir` to the underlying scripts.

## Provenance Defaults

Read verbs (`situational`, `recall`, `why`, `verify`) run under the CLI's
own default actor and record no new events.

Write verbs (`disagree`, `supersede`) pass `--actor agent:<tool>:<session>`
explicitly, derived the same way `hivemind-capture`'s scripts derive it —
`CLAUDE_SESSION_ID`/`CLAUDE_CODE_SESSION_ID` under Claude,
`CODEX_THREAD_ID`/`CODEX_SESSION_ID`/`CODEX_TASK_ID` under Codex, falling
back to Gas City session variables. **Known limitation:** the CLI's
`disagree`/`supersede` commands currently construct `EventProvenance::human`
regardless of the `--actor` value passed
(`src/cli/run/mod.rs::run_disagree`/`run_supersede`), so the emitted
event's `actor_id` correctly reads `agent:...` but its `source` field
still reads `human`. This is existing `hivemind-tenv.1` CLI behavior, out
of this plugin's scope to change; flagged here rather than silently
worked around.

## Verify

Ask what to know before touching a file:

```text
/hivemind-context:situational --paths src/api.rs
```

Ask what was decided:

```text
/hivemind-context:recall "bearer auth on postgres"
```

Check whether it still holds:

```text
/hivemind-context:verify "bearer auth on postgres"
```

If a description matches more than one decision equally well, every verb
above returns a numbered candidate list instead of guessing; write verbs
additionally perform no write in that case. Re-run with `--pick N` or a bare
`#N`.

## Naming

The plugin directory, both `plugin.json` `name` fields, both marketplace
entries, the skill directory, and this README all use the working name
`hivemind-context`. Alex rejected `hivemind-recall` for this role — it
implies knowing an id, which is exactly what this plugin avoids needing.
Two alternatives considered:

- `hivemind-context` (current) — reads as the counterpart to
  `hivemind-capture` (write vs. read/contest); already the name used
  throughout the `hivemind-tenv` epic's bead text.
- `hivemind-fluent` — matches the parent epic's own title ("agent-fluent
  decision interaction"); more abstract as a plugin name.
- `hivemind-consult` — matches this skill's own framing ("teaches WHEN to
  consult past decisions"); reads as an agent action rather than a noun.

Renaming later is mechanical: the plugin directory, the `name` field in
both `plugin.json` files, the two marketplace entries
(`.claude-plugin/marketplace.json`, `.agents/plugins/marketplace.json`),
the skill directory under `skills/`, and the doc/README references above.

## Uninstall

```text
/plugin uninstall hivemind-context@hivemind --prune
```

Remove the marketplace if no other HiveMind plugins are installed:

```text
/plugin marketplace remove hivemind
```
