---
title: Agent Capture
description: Let Claude Code and Codex capture decisions automatically.
---

HiveMind ships installable capture bundles for **Claude Code** and **Codex**. Both
packages teach agents when to preserve a decision and how to call the same
`decision.capture` CLI used by humans.

## Claude Code

### Install via marketplace

```text
/plugin marketplace add alexknips/hivemind
/plugin install hivemind-capture@hivemind
/reload-plugins
```

This installs three skills and an MCP server:

| Skill / Tool | What it does |
|-------------|-------------|
| `/hivemind-capture:capture-decision` | Capture a decision to the local ledger |
| `/hivemind-capture:query-decisions` | Query recent decisions by topic or status |
| `hivemind` MCP server | Full write + query access via MCP tools |

### Manual install

Copy the skill directory into Claude Code's skill path:

```bash
cp -r plugins/hivemind-capture/skills/hivemind-capture \
    "$HOME/.claude/plugins/hivemind-capture/"
```

### From a repo checkout

In `.claude/settings.json`, the marketplace is pre-configured for trusted sessions
in the HiveMind repository. Claude Code will prompt to install the plugin on launch.

## Codex

From a local HiveMind checkout: open `/plugins`, choose `HiveMind Plugins`, and
install `HiveMind Capture`.

From another checkout or machine, add the repository as a marketplace first:

```bash
codex plugin marketplace add https://github.com/alexknips/hivemind.git
```

Or copy the skill bundle directly:

```bash
cp -r plugins/hivemind-capture/skills/hivemind-capture "$HOME/.agents/skills/"
```

## The capture path

Both bundles call the same noninteractive `decision.capture` CLI path, which:
- Defaults actor and provenance to `agent:<tool>:<session>`
- Records `source=agent` with a per-session `source_ref`
- Writes to `HIVEMIND_DIR` (or `--hivemind-dir`)

```bash
hivemind --hivemind-dir ./hivemind/ emit decision.capture \
  --title "Use direct CLI capture for agent decisions" \
  --rationale "The local command is deterministic and does not depend on hooks" \
  --topic-keys agents,capture \
  --options direct-cli,mcp \
  --chose direct-cli
```

## Keyless classification — no ANTHROPIC_API_KEY required

`ANTHROPIC_API_KEY` is **optional**. Without it the server runs without the
background classifier. Classification still happens on demand via
`/hivemind-capture:classify-queue` — **Worker A**, which uses your agent's
subscription seat.

After a capture session:

```text
/hivemind-capture:classify-queue
```

The command drains the pending classification queue and writes
`IngestBatchClassified` events. Pass `--limit N` to cap the number of batches
per run (default 20).

This means you can self-host HiveMind with zero external API keys and still get
fully classified decisions — you just run the classify command rather than
relying on the server's background worker.

See the [keyless capture walkthrough](https://github.com/alexknips/hivemind/blob/master/docs/KEYLESS_CAPTURE.md)
for a zero-to-first-decision guide.

## What agents should capture

The capture classifier (layer 3) helps distinguish signal from noise. In practice:

**Capture:**
- A choice between implementation approaches where the rationale is non-obvious
- A decision that future agents or humans will need to understand
- A reversal of a previous decision (use `--supersedes <id>`)

**Don't capture:**
- Mechanical steps (reading a file, running a test)
- Exploratory actions with no committed outcome
- Status updates or progress notes

The classifier runs in production on the HiveMind development process itself and
correctly classifies ~92% of agent activity as noise. The 8% that becomes a captured
decision is the part that matters six months later.

## Reviewing agent decisions

Humans review recent agent decisions in a guided terminal flow:

```bash
hivemind --actor human:lead --hivemind-dir ./hivemind review \
  --actor 'agent:*' \
  --since 7d \
  --unreviewed-only
```

- **Approve** → appends `decision.accepted`
- **Disagree** → appends `decision.rejected` with the reason
- **Supersede** → proposes a replacement decision plus `decision.superseded`

Reviewed/unreviewed state is derived from the reviewer's explicit write events,
not from a separate review flag.
