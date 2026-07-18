# Keyless Capture — Zero to First Decision

Capture and classify your first decision without an `ANTHROPIC_API_KEY`.
The server-side classifier (Worker B) is optional — Worker A drains the
same queue using your agent's subscription seat.

---

## Step 1 — Start a HiveMind cell

**Hosted (no install):** Connect the managed cell via MCP:

```bash
claude mcp add --transport http hivemind https://hivemind-tti3sa.fly.dev/mcp
```

Sign in with GitHub or Google. Skip to Step 2.

**Self-hosted:** Clone and start the cell in three commands:

```bash
git clone https://github.com/alexknips/hivemind && cd hivemind
cp .env.example .env
# Linux:
sed -i "s/change-me-before-production/$(openssl rand -hex 32)/g" .env
# macOS:
# sed -i '' "s/change-me-before-production/$(openssl rand -hex 32)/g" .env
docker compose up --build -d
```

No `ANTHROPIC_API_KEY` needed in `.env` — leave that line empty or omit it.

Provision your first token:

```bash
ADMIN_KEY=$(grep HIVEMIND_ADMIN_KEY .env | cut -d= -f2)
TOKEN=$(curl -s -X POST http://localhost:8080/v1/tenants \
  -H "Authorization: Bearer $ADMIN_KEY" \
  -H "Content-Type: application/json" \
  -d '{"tenant_id": "me", "display_name": "Me"}' | jq -r '.token_secret')
echo "$TOKEN"   # hm_tk_...
```

---

## Step 2 — Install the hivemind-capture plugin

From Claude Code:

```text
/plugin marketplace add alexknips/hivemind
/plugin install hivemind-capture@hivemind
/reload-plugins
```

The plugin adds `/hivemind-capture:capture`, `/hivemind-capture:query-decisions`,
`/hivemind-capture:classify-queue`, and a bundled MCP server.

For Codex, install from the repository:

```bash
codex plugin marketplace add https://github.com/alexknips/hivemind.git
```

Then open `/plugins → HiveMind Plugins → HiveMind Capture`.

---

## Step 3 — Capture a decision in-session

From Claude Code, call the `/capture` command during a live session:

```text
/hivemind-capture:capture "Switch to Postgres for the shared backend" \
  --kind decision \
  --title "Switch to Postgres for the shared backend" \
  --rationale "SQLite WAL mode does not scale across concurrent writers" \
  --topic-keys infrastructure,storage \
  --options sqlite,postgres \
  --chose postgres
```

The command writes immediately to the local ledger (or the remote server if
the bundled MCP server is configured) and prints a one-line confirmation:

```
✓ captured decision:<id>  — query: /hivemind-capture:query-decisions --limit 5
```

The decision is in the ledger. The classification queue now has an unclassified
ingest batch waiting for topic annotation.

---

## Step 4 — Drain the queue with Worker A (no API key)

Run the classify-queue command after your session:

```text
/hivemind-capture:classify-queue
```

This uses your subscription model seat — no `ANTHROPIC_API_KEY` required.
The command:

1. Lists pending batches in the classification queue
2. Classifies each batch using your agent's model
3. Writes `IngestBatchClassified` events to the ledger
4. Reports batches processed and total captures written

Pass `--limit N` to cap the number of batches per run (default 20):

```text
/hivemind-capture:classify-queue --limit 5
```

Check queue depth at any time:

```bash
hivemind classify-queue list --json | jq length
```

---

## Step 5 — Query it back

```text
/hivemind-capture:query-decisions --limit 5
```

Or via CLI:

```bash
hivemind query recent_decisions --since 1h --limit 5
hivemind query get_relevant_decisions --topic infrastructure
hivemind query search_decisions --q "postgres"
```

---

## How the two classifiers relate

| | Worker A | Worker B |
|---|---|---|
| **Where** | Agent-side (`/classify-queue`) | Server-side (`src/classifier.rs`) |
| **Key required** | No — uses subscription seat | Yes — `ANTHROPIC_API_KEY` |
| **When it runs** | On demand after your session | Automatically in background |
| **Queue** | Same shared work queue | Same shared work queue |
| **Result** | Identical `IngestBatchClassified` events | Identical `IngestBatchClassified` events |

Both workers write to the same queue and the result is the same — concurrent
classification is idempotent (last writer wins per batch).

If you later add `ANTHROPIC_API_KEY` to the server, Worker B starts classifying
automatically. Worker A is still available for on-demand draining or when the
server key is absent.

---

## See also

- [`docs/SELF_HOSTING.md`](SELF_HOSTING.md) — full self-host runbook
- [`plugins/hivemind-capture/README.md`](../plugins/hivemind-capture/README.md) — plugin install, commands, and session hooks
- [`docs/AGENT_DECISION_CAPTURE.md`](AGENT_DECISION_CAPTURE.md) — all capture paths (CLI, HTTP API, hooks, MCP)
- [`docs/CAPTURE_CLASSIFIER.md`](CAPTURE_CLASSIFIER.md) — classifier design and queue mechanics
