#!/usr/bin/env bash
# End-to-end smoke test for HiveMind — drives a real running HTTP server.
#
# Usage:
#   ./scripts/e2e_smoke.sh [--base-url URL] [--api-key KEY] [--tenant TENANT]
#
# Environment overrides (lowest precedence; flags win):
#   HIVEMIND_E2E_BASE_URL   — default: http://localhost:8080
#   HIVEMIND_E2E_API_KEY    — default: (empty — dev/no-auth mode)
#   HIVEMIND_E2E_TENANT     — default: e2e-test
#   HIVEMIND_BIN            — path to hivemind binary (for CLI + MCP legs)
#   FIDELITY_BIN            — path to fidelity-eval binary (for ceiling-mode smoke)
#   FIDELITY_CORPUS         — path to fidelity corpus YAML (default: benchmarks/fidelity/corpus.yaml)
#   FIDELITY_CORPUS_SMOKE   — path to mini smoke corpus (default: benchmarks/fidelity/corpus-smoke.yaml)
#   CLAUDE_BIN              — path to the `claude` CLI (default: claude). Used only by the
#                             LOCAL-only "LLM-gated (subscription/edge path)" section below.
#   ANTHROPIC_API_KEY       — if SET, the subscription/edge LLM legs are SKIPPED (they
#                             specifically test the keyless path; the server-side
#                             classifier/scorer workers in src/classifier.rs and
#                             src/scorer.rs are a separate, still-keyed mechanism and are
#                             not exercised by this script at all — see hivemind-fabq.3).
#
# Exit codes: 0 = all checks passed; 1 = at least one check failed.
#
# Minimal runtime deps: curl, jq.  hivemind binary for CLI/MCP legs.
# The script does NOT start or stop the server — call it after compose is up.
#
# The "Fluent CLI leg (in-container, both backends)" section below shells out to
# `docker compose exec hivemind hivemind ...` to run the CLI against whichever
# backend the running compose stack is wired to (HIVEMIND_DIR/HIVEMIND_DATABASE_URL
# already set in the container's own environment — this script never branches on
# backend). It SKIPs cleanly when no `docker compose`-managed `hivemind` service is
# running (e.g. a bare `cargo run` local server) — see hivemind-ot72.13a.

set -euo pipefail

# ── defaults ──────────────────────────────────────────────────────────────────
BASE_URL="${HIVEMIND_E2E_BASE_URL:-http://localhost:8080}"
API_KEY="${HIVEMIND_E2E_API_KEY:-}"
API_KEY_B="${HIVEMIND_E2E_API_KEY_B:-}"  # separate token for tenant-B in auth mode
TENANT="${HIVEMIND_E2E_TENANT:-e2e-test}"
TENANT_B="e2e-test-other"  # fixed second tenant for multi-tenant isolation
HIVEMIND_BIN="${HIVEMIND_BIN:-hivemind}"
SKIP_MAP="${HIVEMIND_E2E_SKIP_MAP:-false}"     # flag kept for local convenience; map works on all backends since 270r
SKIP_SEARCH="${HIVEMIND_E2E_SKIP_SEARCH:-false}"  # set true for Postgres (FTS not in shared-backend)
FIDELITY_BIN="${FIDELITY_BIN:-fidelity-eval}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
FIDELITY_CORPUS="${FIDELITY_CORPUS:-$REPO_ROOT/benchmarks/fidelity/corpus.yaml}"
FIDELITY_CORPUS_SMOKE="${FIDELITY_CORPUS_SMOKE:-$REPO_ROOT/benchmarks/fidelity/corpus-smoke.yaml}"

# ── arg parsing ───────────────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
  case "$1" in
    --base-url)  BASE_URL="$2";  shift 2 ;;
    --api-key)   API_KEY="$2";   shift 2 ;;
    --tenant)    TENANT="$2";    shift 2 ;;
    --skip-map)    SKIP_MAP=true;    shift ;;
    --skip-search) SKIP_SEARCH=true; shift ;;
    *) echo "unknown arg: $1" >&2; exit 1 ;;
  esac
done

# ── helpers ───────────────────────────────────────────────────────────────────
PASS=0
FAIL=0
SKIP=0

pass() { echo "  PASS  $1"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL  $1"; FAIL=$((FAIL + 1)); }
skip() { echo "  SKIP  $1"; SKIP=$((SKIP + 1)); }

section() { echo; echo "── $1 ──"; }

auth_header() {
  if [[ -n "$API_KEY" ]]; then
    printf '%s' "-H 'Authorization: Bearer $API_KEY'"
  fi
}

curl_api() {
  local method="$1"; shift
  local path="$1";   shift
  local extra=("$@")
  local auth_args=()
  [[ -n "$API_KEY" ]] && auth_args+=(-H "Authorization: Bearer $API_KEY")
  curl -s \
    -H "X-HiveMind-Tenant: $TENANT" \
    -H "X-HiveMind-Actor: agent:e2e:smoke" \
    "${auth_args[@]}" \
    -X "$method" \
    "${extra[@]}" \
    "$BASE_URL$path"
}

curl_json() {
  local method="$1"; shift
  local path="$1";   shift
  local body="$1";   shift
  local auth_args=()
  [[ -n "$API_KEY" ]] && auth_args+=(-H "Authorization: Bearer $API_KEY")
  curl -s \
    -H "Content-Type: application/json" \
    -H "X-HiveMind-Tenant: $TENANT" \
    -H "X-HiveMind-Actor: agent:e2e:smoke" \
    "${auth_args[@]}" \
    -X "$method" \
    -d "$body" \
    "$BASE_URL$path"
}

# ── tenant provisioning (dev/no-auth SQLite backend only) ──────────────────────
# Unknown --tenant / X-HiveMind-Tenant now hard-errors at the ledger-open seam
# (hivemind-rkbf) instead of silently opening a fresh scope. The Postgres CI
# job provisions tenant A + B itself via the admin-gated POST /v1/tenants
# route before this script runs (see ci.yml) — that route only exists with
# the shared-backend-postgres feature. A SQLite server has no HTTP
# provisioning path, so register its tenants directly against its ledger via
# `docker compose exec` instead. Runs only in dev/no-auth mode (an API key
# implies Postgres/auth mode, already provisioned externally) and only when a
# compose-managed `hivemind` service is actually running; a bare local server
# (e.g. `cargo run`) is assumed to already have its tenant(s) registered.
if [[ -z "$API_KEY" ]] && command -v docker > /dev/null 2>&1 \
   && docker compose ps --status running --services 2>/dev/null | grep -qx hivemind; then
  for t in "$TENANT" "$TENANT_B"; do
    if ! out=$(docker compose exec -T hivemind hivemind tenant create "$t" 2>&1); then
      echo "WARNING: failed to provision SQLite tenant '$t': $out" >&2
    fi
  done
fi

# ── health check ──────────────────────────────────────────────────────────────
section "Health"
if curl -sf "$BASE_URL/v1/health" | jq -e '.status == "ok"' > /dev/null 2>&1; then
  pass "GET /v1/health returns {status:ok}"
else
  fail "GET /v1/health — server not ready or wrong response"
fi

# ── capture via HTTP ──────────────────────────────────────────────────────────
section "Capture — HTTP"

# still_proposed: this decision gets disagreed with later in this script by
# the same actor ("Review — disagree + supersede"). Self-accept-by-default
# (hivemind-zdsh.8) would make this actor the acceptor, and the ledger
# invariant refuses one actor both accepting and rejecting the same decision
# (a changed mind is a supersede, not a disagreement with oneself — mayor
# ruling on hivemind-zdsh.8, option c). Keeping it proposed here makes the
# later disagree a legitimate lone rejection instead.
decision_resp=$(curl_json POST /v1/decisions '{
  "title": "e2e-smoke: use Postgres for shared storage",
  "rationale": "SQLite is single-writer; Postgres supports concurrent tenants",
  "topic_keys": ["storage", "e2e"],
  "options": [
    {"label": "postgres", "description": "Postgres with connection pool"},
    {"label": "sqlite",   "description": "SQLite WAL mode"}
  ],
  "chosen_option_label": "postgres",
  "still_proposed": true,
  "grounding": [
    {"kind": "evidence", "content": "SQLite allows one writer at a time", "source": "https://www.sqlite.org/lockingv3.html"}
  ]
}')

if echo "$decision_resp" | jq -e '.decision_id' > /dev/null 2>&1; then
  DECISION_ID=$(echo "$decision_resp" | jq -r '.decision_id')
  pass "POST /v1/decisions — captured decision $DECISION_ID"
else
  fail "POST /v1/decisions — unexpected response: $decision_resp"
  DECISION_ID=""
fi

evidence_resp=$(curl_json POST /v1/evidence '{
  "content": "e2e smoke: load test at 200 concurrent tenants sustained",
  "source": "e2e-smoke"
}')
if echo "$evidence_resp" | jq -e '.evidence_id' > /dev/null 2>&1; then
  EVIDENCE_ID=$(echo "$evidence_resp" | jq -r '.evidence_id')
  pass "POST /v1/evidence — captured $EVIDENCE_ID"
else
  fail "POST /v1/evidence — unexpected response: $evidence_resp"
  EVIDENCE_ID=""
fi

hypothesis_resp=$(curl_json POST /v1/hypotheses '{
  "statement": "e2e smoke: Postgres connection pool handles burst load without queueing"
}')
if echo "$hypothesis_resp" | jq -e '.hypothesis_id' > /dev/null 2>&1; then
  HYPOTHESIS_ID=$(echo "$hypothesis_resp" | jq -r '.hypothesis_id')
  pass "POST /v1/hypotheses — captured $HYPOTHESIS_ID"
else
  fail "POST /v1/hypotheses — unexpected response: $hypothesis_resp"
  HYPOTHESIS_ID=""
fi

# ── capture via CLI ───────────────────────────────────────────────────────────
section "Capture — CLI"

CLI_DATA_DIR=$(mktemp -d)
trap 'rm -rf "$CLI_DATA_DIR" "${EDGE_DIR:-}" "${CONTEXT_DIR:-}"' EXIT

if command -v "$HIVEMIND_BIN" > /dev/null 2>&1; then
  # --project-from-context (what the capture plugins pass) works the project out from the
  # working directory: the nearest .hivemind-project walking up. The captures below run from
  # folders under the throwaway ledger dir, so this repository's own marker (a project this
  # ledger has never registered) can never decide them; the binary path is made absolute
  # because CI hands us a relative one.
  CLI_BIN=$(command -v "$HIVEMIND_BIN")
  case "$CLI_BIN" in /*) ;; *) CLI_BIN="$PWD/$CLI_BIN" ;; esac
  CLI_BARE_DIR="$CLI_DATA_DIR/bare"
  mkdir -p "$CLI_BARE_DIR"

  # Fresh --hivemind-dir only seeds the "local" default tenant; register
  # ours before the emit below hits the unknown-tenant hard error.
  "$CLI_BIN" --hivemind-dir "$CLI_DATA_DIR" tenant create "$TENANT" > /dev/null
  cli_out=$(cd "$CLI_BARE_DIR" && "$CLI_BIN" \
    --hivemind-dir "$CLI_DATA_DIR" \
    --actor "agent:e2e:smoke-cli" \
    --tenant "$TENANT" \
    --json \
    emit decision.proposed \
    --project-from-context \
    --title "e2e-smoke CLI: adopt semantic versioning" \
    --rationale "Semver gives downstream consumers predictable upgrade signals" \
    --options semver,calver \
    --chose semver \
    --topic-keys e2e,versioning 2>&1) || true
  if echo "$cli_out" | jq -e 'select(.kind=="decision_id" and (.value | startswith("decision-")))' > /dev/null 2>&1; then
    CLI_DECISION_ID=$(echo "$cli_out" | jq -r '.value')
    pass "CLI (local ledger): emit decision.proposed — $CLI_DECISION_ID"
  elif echo "$cli_out" | jq -e '.decision_id' > /dev/null 2>&1; then
    CLI_DECISION_ID=$(echo "$cli_out" | jq -r '.decision_id')
    pass "CLI (local ledger): emit decision.proposed — $CLI_DECISION_ID"
  else
    fail "CLI (local ledger): emit decision.proposed — output: $cli_out"
  fi

  # The reply names the project the capture landed in. Nothing here attaches the folder, so it
  # is the personal project — announced, with the reminder to attach the folder.
  if echo "$cli_out" | jq -e '.project_source == "personal_fallback" and (.project | type == "string") and (.project_notice | type == "string") and (.project_reminder | type == "string")' > /dev/null 2>&1; then
    pass "CLI (local ledger): project: $(echo "$cli_out" | jq -r '.project + " (" + .project_source + ")"') — personal-project notice and attach-the-folder reminder present"
  else
    fail "CLI (local ledger): capture from an unattached folder did not name its personal project with notice and reminder — output: $cli_out"
  fi

  # A folder attached to a registered project files the capture there: no fallback, no reminder.
  "$CLI_BIN" --hivemind-dir "$CLI_DATA_DIR" --actor "agent:e2e:smoke-cli" --tenant "$TENANT" \
    project register e2e-smoke-project > /dev/null 2>&1 || true
  # A registered project's topic keys are declared (hivemind-zywz); the capture below uses these.
  "$CLI_BIN" --hivemind-dir "$CLI_DATA_DIR" --actor "agent:e2e:smoke-cli" --tenant "$TENANT" \
    project declare-topic e2e-smoke-project e2e tooling > /dev/null 2>&1 || true
  mkdir -p "$CLI_DATA_DIR/attached/src"
  printf 'e2e-smoke-project\n' > "$CLI_DATA_DIR/attached/.hivemind-project"
  cli_marker_out=$(cd "$CLI_DATA_DIR/attached/src" && "$CLI_BIN" \
    --hivemind-dir "$CLI_DATA_DIR" \
    --actor "agent:e2e:smoke-cli" \
    --tenant "$TENANT" \
    --json \
    emit decision.proposed \
    --project-from-context \
    --title "e2e-smoke CLI: pin the release tooling version" \
    --rationale "A pinned toolchain makes release builds reproducible across machines" \
    --options pinned,floating \
    --chose pinned \
    --topic-keys e2e,tooling 2>&1) || true
  if echo "$cli_marker_out" | jq -e '.project == "e2e-smoke-project" and .project_source == "folder_marker" and (has("project_reminder") | not)' > /dev/null 2>&1; then
    pass "CLI (local ledger): project: e2e-smoke-project (folder_marker) — captured from a marked folder, no reminder"
  else
    fail "CLI (local ledger): capture from a marked folder did not land in its project — output: $cli_marker_out"
  fi
else
  skip "CLI leg — hivemind binary not on PATH (set HIVEMIND_BIN)"
fi

# ── capture via MCP HTTP ──────────────────────────────────────────────────────
section "Capture — MCP HTTP"

MCP_ACTOR="agent:e2e:smoke-mcp"
mcp_auth_args=()
[[ -n "$API_KEY" ]] && mcp_auth_args+=(-H "Authorization: Bearer $API_KEY")

# initialize — obtains Mcp-Session-Id for subsequent requests
mcp_init_headers=$(mktemp)
curl -s \
  -H "Content-Type: application/json" \
  -H "X-HiveMind-Tenant: $TENANT" \
  -H "X-HiveMind-Actor: $MCP_ACTOR" \
  "${mcp_auth_args[@]}" \
  -X POST \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' \
  -D "$mcp_init_headers" \
  "$BASE_URL/mcp" > /dev/null
MCP_SESSION_ID=$(grep -i '^mcp-session-id:' "$mcp_init_headers" | tr -d '\r' | awk '{print $2}')
rm -f "$mcp_init_headers"

mcp_session_args=()
[[ -n "$MCP_SESSION_ID" ]] && mcp_session_args+=(-H "Mcp-Session-Id: $MCP_SESSION_ID")

# tools/call capture_decision against the live server
mcp_capture=$(curl -s \
  -H "Content-Type: application/json" \
  -H "X-HiveMind-Tenant: $TENANT" \
  -H "X-HiveMind-Actor: $MCP_ACTOR" \
  "${mcp_auth_args[@]}" \
  "${mcp_session_args[@]}" \
  -X POST \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"capture_decision","arguments":{"actor_id":"agent:e2e:smoke-mcp","title":"e2e-smoke MCP: prefer immutable events","rationale":"Immutable append-only log simplifies auditing","topic_keys":["e2e","architecture"],"options":[{"label":"immutable"},{"label":"mutable"}],"chosen_option_label":"immutable","grounding":[{"kind":"assumption","statement":"Auditors need to replay history exactly as it was written"}]}}}' \
  "$BASE_URL/mcp")

if echo "$mcp_capture" | jq -e '.result.content[0].text' > /dev/null 2>&1; then
  mcp_text=$(echo "$mcp_capture" | jq -r '.result.content[0].text')
  if echo "$mcp_text" | jq -e '.decision_id' > /dev/null 2>&1; then
    MCP_DECISION_ID=$(echo "$mcp_text" | jq -r '.decision_id')
    verify_resp=$(curl_api GET "/v1/decisions/$MCP_DECISION_ID")
    if echo "$verify_resp" | jq -e 'has("data")' > /dev/null 2>&1; then
      pass "MCP HTTP capture_decision — $MCP_DECISION_ID (verified via REST)"
    else
      pass "MCP HTTP capture_decision — $MCP_DECISION_ID"
    fi
  else
    fail "MCP HTTP capture_decision — tool text: $mcp_text"
  fi
else
  fail "MCP HTTP capture_decision — unexpected response: $mcp_capture"
fi

# ── fluent disagree via MCP HTTP ────────────────────────────────────────────────
# disagree_decision resolves decision_id | description (hivemind-ot72.6): a
# description matching two decisions at the same confidence tier is a
# successful ambiguous result, never a write; MCP has no #N/--pick session to
# retry against, so the agent re-calls with the decision_id the first call
# already told it about.
section "Fluent disagree — MCP HTTP"

mcp_capture_fluent_a=$(curl -s \
  -H "Content-Type: application/json" \
  -H "X-HiveMind-Tenant: $TENANT" \
  -H "X-HiveMind-Actor: $MCP_ACTOR" \
  "${mcp_auth_args[@]}" \
  "${mcp_session_args[@]}" \
  -X POST \
  -d '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"capture_decision","arguments":{"actor_id":"agent:e2e:smoke-mcp","title":"e2e-smoke MCP fluent: adopt async billing queue","rationale":"Durability beats latency for billing events","topic_keys":["e2e","fluent"],"options":[{"label":"async"}],"grounding":[{"kind":"bet","statement":"Billing volume stays low enough for one queue"}]}}}' \
  "$BASE_URL/mcp")
mcp_capture_fluent_b=$(curl -s \
  -H "Content-Type: application/json" \
  -H "X-HiveMind-Tenant: $TENANT" \
  -H "X-HiveMind-Actor: $MCP_ACTOR" \
  "${mcp_auth_args[@]}" \
  "${mcp_session_args[@]}" \
  -X POST \
  -d '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"capture_decision","arguments":{"actor_id":"agent:e2e:smoke-mcp","title":"e2e-smoke MCP fluent: adopt async notifications queue","rationale":"Durability beats latency for notification events","topic_keys":["e2e","fluent"],"options":[{"label":"async"}],"grounding":[{"kind":"bet","statement":"Notification volume stays low enough for one queue"}]}}}' \
  "$BASE_URL/mcp")

MCP_FLUENT_DECISION_A=""
if echo "$mcp_capture_fluent_a" | jq -e '.result.structuredContent.decision_id' > /dev/null 2>&1; then
  MCP_FLUENT_DECISION_A=$(echo "$mcp_capture_fluent_a" | jq -r '.result.structuredContent.decision_id')
fi
MCP_FLUENT_DECISION_B=""
if echo "$mcp_capture_fluent_b" | jq -e '.result.structuredContent.decision_id' > /dev/null 2>&1; then
  MCP_FLUENT_DECISION_B=$(echo "$mcp_capture_fluent_b" | jq -r '.result.structuredContent.decision_id')
fi

if [[ -n "$MCP_FLUENT_DECISION_A" && -n "$MCP_FLUENT_DECISION_B" ]]; then
  pass "MCP HTTP capture_decision (fluent disagree seed) — 2 decisions captured"

  # Snapshot both candidates' status before the ambiguous call. "No write"
  # means resolution touches neither — a disagreement flips a decision's
  # derived status to contested, so an unchanged status on both sides is
  # the observable proof, the same invariant the in-container CLI leg above
  # checks via ledger offset (no equivalent raw-offset route exists over
  # MCP HTTP).
  status_a_before=$(curl_api GET "/v1/decisions/$MCP_FLUENT_DECISION_A" | jq -r '.data.status // "missing"')
  status_b_before=$(curl_api GET "/v1/decisions/$MCP_FLUENT_DECISION_B" | jq -r '.data.status // "missing"')

  mcp_disagree_ambiguous=$(curl -s \
    -H "Content-Type: application/json" \
    -H "X-HiveMind-Tenant: $TENANT" \
    -H "X-HiveMind-Actor: $MCP_ACTOR" \
    "${mcp_auth_args[@]}" \
    "${mcp_session_args[@]}" \
    -X POST \
    -d '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"disagree_decision","arguments":{"description":"adopt async queue","reason":"e2e smoke: should not resolve to either"}}}' \
    "$BASE_URL/mcp")
  if [[ "$(echo "$mcp_disagree_ambiguous" | jq -r '.result.isError')" == "false" ]] \
    && [[ "$(echo "$mcp_disagree_ambiguous" | jq -r '.result.structuredContent.data.outcome')" == "ambiguous" ]]; then
    pass "MCP HTTP disagree_decision — ambiguous description returns candidates, not an error"
  else
    fail "MCP HTTP disagree_decision — ambiguous description: $mcp_disagree_ambiguous"
  fi

  status_a_after_ambiguous=$(curl_api GET "/v1/decisions/$MCP_FLUENT_DECISION_A" | jq -r '.data.status // "missing"')
  status_b_after_ambiguous=$(curl_api GET "/v1/decisions/$MCP_FLUENT_DECISION_B" | jq -r '.data.status // "missing"')
  if [[ "$status_a_after_ambiguous" == "$status_a_before" && "$status_b_after_ambiguous" == "$status_b_before" ]]; then
    pass "MCP HTTP disagree_decision — ambiguous description wrote to neither candidate (status unchanged: $status_a_before / $status_b_before)"
  else
    fail "MCP HTTP disagree_decision — ambiguous description wrote a status change: a $status_a_before->$status_a_after_ambiguous b $status_b_before->$status_b_after_ambiguous"
  fi

  mcp_disagree_resolved=$(curl -s \
    -H "Content-Type: application/json" \
    -H "X-HiveMind-Tenant: $TENANT" \
    -H "X-HiveMind-Actor: $MCP_ACTOR" \
    "${mcp_auth_args[@]}" \
    "${mcp_session_args[@]}" \
    -X POST \
    -d "{\"jsonrpc\":\"2.0\",\"id\":6,\"method\":\"tools/call\",\"params\":{\"name\":\"disagree_decision\",\"arguments\":{\"decision_id\":\"$MCP_FLUENT_DECISION_A\",\"reason\":\"e2e smoke: disagree via resolved decision_id\"}}}" \
    "$BASE_URL/mcp")
  if [[ "$(echo "$mcp_disagree_resolved" | jq -r '.result.isError')" == "false" ]] \
    && echo "$mcp_disagree_resolved" | jq -e '.result.structuredContent.event_id' > /dev/null 2>&1; then
    pass "MCP HTTP disagree_decision — resolved by decision_id after the ambiguous retry"
  else
    fail "MCP HTTP disagree_decision — resolved by decision_id: $mcp_disagree_resolved"
  fi

  # A lone disagreement with no prior acceptance derives to "rejected", not
  # "contested" — derive_decision_status (src/queries/status.rs) only
  # returns contested when a decision carries BOTH an AcceptedBy and a
  # RejectedBy edge. This decision was only ever proposed, so rejected is
  # the correct post-write status here.
  status_a_after_resolved=$(curl_api GET "/v1/decisions/$MCP_FLUENT_DECISION_A" | jq -r '.data.status // "missing"')
  if [[ "$status_a_after_resolved" == "rejected" ]]; then
    pass "MCP HTTP disagree_decision — decision_id call actually wrote: status $status_a_before -> rejected"
  else
    fail "MCP HTTP disagree_decision — decision_id call did not persist a write: status stayed $status_a_after_resolved"
  fi

  # ── fluent get_decision_neighborhood via MCP HTTP ──────────────────────────
  # get_decision_neighborhood (the CLI's `why`) resolves the same
  # decision_id | description contract disagree_decision does above
  # (hivemind-ot72.5). "billing" is the term that breaks the tie against
  # fluent decision B's title, so this resolves uniquely to decision A
  # instead of retracing the ambiguous path already covered above.
  mcp_neighborhood=$(curl -s \
    -H "Content-Type: application/json" \
    -H "X-HiveMind-Tenant: $TENANT" \
    -H "X-HiveMind-Actor: $MCP_ACTOR" \
    "${mcp_auth_args[@]}" \
    "${mcp_session_args[@]}" \
    -X POST \
    -d '{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"get_decision_neighborhood","arguments":{"description":"adopt async billing queue"}}}' \
    "$BASE_URL/mcp")
  if [[ "$(echo "$mcp_neighborhood" | jq -r '.result.isError')" == "false" ]] \
    && [[ "$(echo "$mcp_neighborhood" | jq -r '.result.structuredContent.data.root.id')" == "$MCP_FLUENT_DECISION_A" ]]; then
    pass "MCP HTTP get_decision_neighborhood — resolves by description to the billing decision"
  else
    fail "MCP HTTP get_decision_neighborhood — resolve by description: $mcp_neighborhood"
  fi
else
  skip "MCP HTTP disagree_decision — ambiguous description returns candidates, not an error"
  skip "MCP HTTP disagree_decision — ambiguous description wrote to neither candidate"
  skip "MCP HTTP disagree_decision — resolved by decision_id after the ambiguous retry"
  skip "MCP HTTP disagree_decision — decision_id call actually wrote a status change"
  skip "MCP HTTP get_decision_neighborhood — resolves by description to the billing decision"
fi

# ── query / projection ────────────────────────────────────────────────────────
section "Query — projection"

if [[ -n "$DECISION_ID" ]]; then
  get_resp=$(curl_api GET "/v1/decisions/$DECISION_ID")
  if echo "$get_resp" | jq -e 'has("data")' > /dev/null 2>&1; then
    pass "GET /v1/decisions/{id} — retrieved $DECISION_ID"
  else
    fail "GET /v1/decisions/{id} — response: $get_resp"
  fi

  compact_resp=$(curl_api GET "/v1/decisions/$DECISION_ID/compact-view")
  if echo "$compact_resp" | jq -e 'has("data")' > /dev/null 2>&1; then
    pass "GET /v1/decisions/{id}/compact-view"
  else
    fail "GET /v1/decisions/{id}/compact-view — response: $compact_resp"
  fi
else
  skip "GET /v1/decisions/{id} — no decision_id (capture failed)"
  skip "GET /v1/decisions/{id}/compact-view — no decision_id"
fi

graph_resp=$(curl_api GET /v1/graph)
if echo "$graph_resp" | jq -e 'has("nodes")' > /dev/null 2>&1; then
  node_count=$(echo "$graph_resp" | jq '.nodes | length')
  pass "GET /v1/graph — $node_count nodes"
else
  fail "GET /v1/graph — response: $graph_resp"
fi

# ── search ────────────────────────────────────────────────────────────────────
section "Search"

if [[ "$SKIP_SEARCH" == "true" ]]; then
  skip "GET /v1/decisions/search — skipped on Postgres backend (FTS not in shared-backend mode)"
else
  search_resp=$(curl_api GET "/v1/decisions/search?q=postgres")
  if echo "$search_resp" | jq -e 'has("data")' > /dev/null 2>&1; then
    count=$(echo "$search_resp" | jq '.data.total_matches // (.data.items | length) // 0')
    pass "GET /v1/decisions/search?q=postgres — $count result(s)"
  else
    fail "GET /v1/decisions/search — response: $search_resp"
  fi
fi

relevant_resp=$(curl_api GET "/v1/decisions/relevant?topic=storage")
if echo "$relevant_resp" | jq -e 'has("data")' > /dev/null 2>&1; then
  pass "GET /v1/decisions/relevant?topic=storage"
else
  fail "GET /v1/decisions/relevant — response: $relevant_resp"
fi

# ── spectral map ──────────────────────────────────────────────────────────────
section "Spectral map"

if [[ "$SKIP_MAP" == "true" ]]; then
  skip "GET /v1/decisions/map — explicitly skipped via --skip-map"
else
  map_resp=$(curl_api GET "/v1/decisions/map" 2>&1 || true)
  if echo "$map_resp" | jq -e 'has("points")' > /dev/null 2>&1; then
    pass "GET /v1/decisions/map"
  else
    fail "GET /v1/decisions/map — response: $map_resp"
  fi
fi

# ── fluent read routes ──────────────────────────────────────────────────────────
# One call per route (existence + shape only — the fluent flow itself is C5).
section "Fluent read routes"

situational_resp=$(curl_api GET "/v1/decisions/situational?paths=storage")
if echo "$situational_resp" | jq -e 'has("data")' > /dev/null 2>&1; then
  pass "GET /v1/decisions/situational?paths=storage"
else
  fail "GET /v1/decisions/situational — response: $situational_resp"
fi

if [[ "$SKIP_SEARCH" == "true" ]]; then
  skip "GET /v1/decisions/recall — skipped (same backend constraint as search)"
else
  recall_resp=$(curl_api GET "/v1/decisions/recall?q=postgres")
  if echo "$recall_resp" | jq -e 'has("data")' > /dev/null 2>&1; then
    pass "GET /v1/decisions/recall?q=postgres"
  else
    fail "GET /v1/decisions/recall — response: $recall_resp"
  fi
fi

if [[ -n "$DECISION_ID" ]]; then
  why_resp=$(curl_api GET "/v1/decisions/why?id=$DECISION_ID")
  if echo "$why_resp" | jq -e 'has("data")' > /dev/null 2>&1; then
    pass "GET /v1/decisions/why?id=<decision>"
  else
    fail "GET /v1/decisions/why — response: $why_resp"
  fi

  verify_resp=$(curl_api GET "/v1/decisions/verify?id=$DECISION_ID")
  if echo "$verify_resp" | jq -e 'has("data")' > /dev/null 2>&1; then
    pass "GET /v1/decisions/verify?id=<decision>"
  else
    fail "GET /v1/decisions/verify — response: $verify_resp"
  fi
else
  skip "GET /v1/decisions/why — no decision_id (capture failed)"
  skip "GET /v1/decisions/verify — no decision_id"
fi

# ── fluent CLI leg — inside the container (BOTH backends) ────────────────────
# Runs the CLI's fluent verbs via `docker compose exec` against the server's OWN
# backend (SQLite or Postgres, whichever HIVEMIND_DATABASE_URL selects inside the
# running container — hivemind-ot72.2) instead of a throwaway local ledger. One
# script body, no backend branching: the container's own environment already
# picked the backend before this script ever runs.
#
# No decision id is ever typed — every read/write below resolves a decision by
# free-text description (+ --pick / --topic), the same fluent contract a coding
# agent uses (docs/AGENT_FLUENT_QUERYING.md). Two decisions share the phrase
# "adopt async retry queue" (ambiguous — 2 candidates) but only the first
# ("...ingestion pipeline") also contains "ingestion" (resolves uniquely).
section "Fluent CLI leg (in-container, both backends)"

CLI_IC_TENANT="$TENANT"
CLI_IC_ACTOR="agent:e2e:smoke-cli-incontainer"
# Disagreement is another actor's act (mayor ruling on hivemind-zdsh.8,
# option c): CLI_IC_ACTOR self-accepts IC_D1 below via --chose, so the actor
# that later disagrees must differ, or the ledger invariant refuses one
# actor both accepting and rejecting the same decision. Using a second actor
# here also makes the disagree a REAL contest (accepted + rejected), not a
# lone rejection, matching what the "contests" assertion below claims.
CLI_IC_REVIEWER="agent:e2e:smoke-cli-incontainer-reviewer"
CLI_IC_TOPIC="smokeclifluent"

hm_ic() {
  docker compose exec -T hivemind hivemind \
    --tenant "$CLI_IC_TENANT" --actor "$CLI_IC_ACTOR" --json "$@" 2>&1
}

hm_ic_reviewer() {
  docker compose exec -T hivemind hivemind \
    --tenant "$CLI_IC_TENANT" --actor "$CLI_IC_REVIEWER" --json "$@" 2>&1
}

if ! command -v docker > /dev/null 2>&1 \
   || ! docker compose ps --status running --services 2>/dev/null | grep -qx hivemind; then
  skip "CLI (in-container): emit decision.proposed — ingestion decision — no running 'hivemind' compose service"
  skip "CLI (in-container): emit decision.proposed — notification decision — no running 'hivemind' compose service"
  skip "CLI (in-container): query situational — no running 'hivemind' compose service"
  skip "CLI (in-container): query recall — no running 'hivemind' compose service"
  skip "CLI (in-container): query why — no running 'hivemind' compose service"
  skip "CLI (in-container): query verify (pre-supersede) — no running 'hivemind' compose service"
  skip "CLI (in-container): disagree (ambiguous description) — no running 'hivemind' compose service"
  skip "CLI (in-container): ledger offset unmoved after ambiguous disagree — no running 'hivemind' compose service"
  skip "CLI (in-container): disagree --pick 2 — no running 'hivemind' compose service"
  skip "CLI (in-container): supersede --pick 2 — no running 'hivemind' compose service"
  skip "CLI (in-container): query verify (post-supersede) — no running 'hivemind' compose service"
else
  # --project-from-context is what the capture plugins pass. The container's working directory
  # has no .hivemind-project above it and no rig, so this lands in the actor's personal project
  # on whichever backend the stack runs -- and the reply must say so.
  ic_d1=$(hm_ic emit decision.proposed \
    --project-from-context \
    --title "Adopt async retry queue for the ingestion pipeline" \
    --rationale "Bounded retries avoid unbounded backlog growth under load" \
    --topic-keys "$CLI_IC_TOPIC" \
    --options "async,sync" \
    --chose "async") || true
  if echo "$ic_d1" | jq -e 'select(.kind=="decision_id")' > /dev/null 2>&1; then
    IC_D1_ID=$(echo "$ic_d1" | jq -r '.value')
    pass "CLI (in-container): emit decision.proposed — ingestion decision ($IC_D1_ID)"
    if echo "$ic_d1" | jq -e '.project_source == "personal_fallback" and (.project | type == "string") and (.project_notice | type == "string") and (.project_reminder | type == "string")' > /dev/null 2>&1; then
      pass "CLI (in-container): project: $(echo "$ic_d1" | jq -r '.project + " (" + .project_source + ")"') — personal-project notice and attach-the-folder reminder present"
    else
      fail "CLI (in-container): capture did not name its personal project with notice and reminder — response: $ic_d1"
    fi
  else
    fail "CLI (in-container): emit decision.proposed — ingestion decision — response: $ic_d1"
    IC_D1_ID=""
  fi

  ic_d2=$(hm_ic emit decision.proposed \
    --title "Adopt async retry queue for the notification pipeline" \
    --rationale "Consistent retry semantics simplify downstream alerting" \
    --topic-keys "$CLI_IC_TOPIC" \
    --options "async") || true
  if echo "$ic_d2" | jq -e 'select(.kind=="decision_id")' > /dev/null 2>&1; then
    IC_D2_ID=$(echo "$ic_d2" | jq -r '.value')
    pass "CLI (in-container): emit decision.proposed — notification decision ($IC_D2_ID)"
  else
    fail "CLI (in-container): emit decision.proposed — notification decision — response: $ic_d2"
    IC_D2_ID=""
  fi

  ic_situational=$(hm_ic query situational --paths "$CLI_IC_TOPIC") || true
  if echo "$ic_situational" | jq -e '.data.total_matches >= 2' > /dev/null 2>&1; then
    pass "CLI (in-container): query situational --paths=$CLI_IC_TOPIC — both decisions surfaced"
  else
    fail "CLI (in-container): query situational — response: $ic_situational"
  fi

  ic_recall=$(hm_ic query recall "adopt async retry queue") || true
  if echo "$ic_recall" | jq -e '(.data.ranked.items | length) >= 2' > /dev/null 2>&1; then
    pass "CLI (in-container): query recall — ranked items include both decisions"
  else
    fail "CLI (in-container): query recall — response: $ic_recall"
  fi

  ic_why=$(hm_ic query why "adopt async retry queue ingestion") || true
  if echo "$ic_why" | jq -e --arg id "$IC_D1_ID" '.data.root.id == $id' > /dev/null 2>&1; then
    pass "CLI (in-container): query why — resolves uniquely to the ingestion decision"
  else
    fail "CLI (in-container): query why — response: $ic_why"
  fi

  ic_verify_pre=$(hm_ic query verify "adopt async retry queue ingestion") || true
  if echo "$ic_verify_pre" | jq -e --arg id "$IC_D1_ID" \
      '.data.decision_id == $id and .data.still_holds.held_up == true' > /dev/null 2>&1; then
    pass "CLI (in-container): query verify — ingestion decision still holds (pre-supersede)"
  else
    fail "CLI (in-container): query verify (pre-supersede) — response: $ic_verify_pre"
  fi

  ic_offset_before=$(hm_ic query get_recent_activity --limit 1) || true
  IC_OFFSET_BEFORE=$(echo "$ic_offset_before" | jq -r '.data.items[0].event_origin // empty')

  ic_disagree_ambig=$(hm_ic_reviewer disagree "adopt async retry queue" \
    --reason "e2e smoke: ambiguous — must not resolve or write") || true
  if echo "$ic_disagree_ambig" | jq -e \
      '.data.outcome == "ambiguous" and (.data.candidates | length) == 2' > /dev/null 2>&1; then
    pass "CLI (in-container): disagree (ambiguous description) — short-circuits with 2 candidates, no write"
  else
    fail "CLI (in-container): disagree (ambiguous) — response: $ic_disagree_ambig"
  fi

  ic_offset_after=$(hm_ic query get_recent_activity --limit 1) || true
  IC_OFFSET_AFTER=$(echo "$ic_offset_after" | jq -r '.data.items[0].event_origin // empty')
  if [[ -n "$IC_OFFSET_BEFORE" && "$IC_OFFSET_AFTER" == "$IC_OFFSET_BEFORE" ]]; then
    pass "CLI (in-container): ledger offset unmoved after ambiguous disagree short-circuit ($IC_OFFSET_BEFORE)"
  else
    fail "CLI (in-container): ledger offset moved after ambiguous disagree — before=$IC_OFFSET_BEFORE after=$IC_OFFSET_AFTER"
  fi

  ic_disagree_pick=$(hm_ic_reviewer disagree "adopt async retry queue" --pick 2 \
    --reason "e2e smoke: retries hide a slower systemic bottleneck") || true
  if echo "$ic_disagree_pick" | jq -e --arg id "$IC_D1_ID" '.decision_id == $id' > /dev/null 2>&1; then
    pass "CLI (in-container): disagree --pick 2 — resolves and contests the ingestion decision"
  else
    fail "CLI (in-container): disagree --pick 2 — response: $ic_disagree_pick"
  fi

  ic_supersede=$(hm_ic supersede "adopt async retry queue" --pick 2 \
    --title "Batch ingestion writes instead of per-event retries" \
    --rationale "e2e smoke: batching removes the retry queue's backlog risk entirely" \
    --options "batched-writes" \
    --chose "batched-writes" \
    --rests-on-assumption "Batching removes the retry backlog risk") || true
  if echo "$ic_supersede" | jq -e --arg id "$IC_D1_ID" \
      '.old_decision_id == $id and .old_decision_status == "superseded"' > /dev/null 2>&1; then
    pass "CLI (in-container): supersede --pick 2 — supersedes the ingestion decision"
  else
    fail "CLI (in-container): supersede --pick 2 — response: $ic_supersede"
  fi

  ic_verify_post=$(hm_ic query verify "adopt async retry queue ingestion") || true
  if echo "$ic_verify_post" | jq -e --arg id "$IC_D1_ID" \
      '.data.decision_id == $id and .data.still_holds.held_up == false' > /dev/null 2>&1; then
    pass "CLI (in-container): query verify — ingestion decision no longer holds (still_holds.held_up=false, post-supersede)"
  else
    fail "CLI (in-container): query verify (post-supersede) — response: $ic_verify_post"
  fi
fi

# ── review operations ─────────────────────────────────────────────────────────
section "Review — disagree + supersede"

if [[ -n "$DECISION_ID" ]]; then
  # Capture a second decision to supersede (options required and must be non-empty)
  d2_resp=$(curl_json POST /v1/decisions '{
    "title": "e2e-smoke: use MySQL instead (superseded)",
    "rationale": "Initial idea before Postgres was chosen",
    "topic_keys": ["storage", "e2e"],
    "options": [{"label": "mysql"}],
    "grounding": [{"kind": "bet", "statement": "MySQL is the database we already know"}]
  }')
  D2_ID=$(echo "$d2_resp" | jq -r '.decision_id // empty')

  disagree_resp=$(curl_json POST "/v1/decisions/$DECISION_ID/disagreements" \
    '{"reason": "e2e smoke: disagree for test purposes"}')
  if echo "$disagree_resp" | jq -e '.event_id' > /dev/null 2>&1; then
    pass "POST /v1/decisions/{id}/disagreements"
  else
    fail "POST /v1/decisions/{id}/disagreements — response: $disagree_resp"
  fi

  if [[ -n "$D2_ID" ]]; then
    # Supersede D2 by creating a new replacement decision (title+rationale for the NEW decision)
    supersede_resp=$(curl_json POST "/v1/decisions/$D2_ID/supersessions" \
      '{"title": "e2e-smoke: Postgres selected over MySQL", "rationale": "e2e smoke: Postgres chosen over MySQL after evaluation", "grounding": [{"kind": "evidence", "content": "Evaluation found MySQL lacks the concurrent tenant story", "source": "e2e smoke"}]}')
    if echo "$supersede_resp" | jq -e '.new_decision_id' > /dev/null 2>&1; then
      pass "POST /v1/decisions/{id}/supersessions"

      chain_resp=$(curl_api GET "/v1/decisions/$D2_ID/supersession-chain")
      if echo "$chain_resp" | jq -e 'has("data")' > /dev/null 2>&1; then
        pass "GET /v1/decisions/{id}/supersession-chain"
      else
        fail "GET /v1/decisions/{id}/supersession-chain — response: $chain_resp"
      fi
    else
      fail "POST /v1/decisions/{id}/supersessions — response: $supersede_resp"
    fi
  else
    skip "POST /v1/decisions/{id}/supersessions — second capture failed"
    skip "GET /v1/decisions/{id}/supersession-chain — supersession not created"
  fi
else
  skip "Review legs — no decision_id (capture failed)"
  skip "Supersession legs — no decision_id"
  skip "Supersession chain — no decision_id"
fi

# ── multi-tenant isolation ────────────────────────────────────────────────────
section "Multi-tenant isolation"

curl_json_tenant_b() {
  local method="$1"; shift
  local path="$1";   shift
  local body="$1";   shift
  local auth_args=()
  # In auth mode, use tenant-B's own token so Postgres RLS isolates correctly.
  # Falls back to API_KEY in SQLite mode where the header drives isolation.
  local key_b="${API_KEY_B:-$API_KEY}"
  [[ -n "$key_b" ]] && auth_args+=(-H "Authorization: Bearer $key_b")
  curl -s \
    -H "Content-Type: application/json" \
    -H "X-HiveMind-Tenant: $TENANT_B" \
    -H "X-HiveMind-Actor: agent:e2e:smoke-b" \
    "${auth_args[@]}" \
    -X "$method" \
    -d "$body" \
    "$BASE_URL$path"
}

tb_resp=$(curl_json_tenant_b POST /v1/decisions \
  '{"title": "e2e-smoke tenant-B only decision", "rationale": "should not appear in tenant A", "topic_keys":["e2e"], "options":[{"label":"opt-b"}], "grounding":[{"kind":"bet"}]}')
if echo "$tb_resp" | jq -e '.decision_id' > /dev/null 2>&1; then
  TB_DECISION_ID=$(echo "$tb_resp" | jq -r '.decision_id')
  # Verify it's invisible from tenant A's graph
  graph_a=$(curl_api GET /v1/graph)
  if echo "$graph_a" | jq -e --arg id "$TB_DECISION_ID" \
      '[.nodes[] | select(.id == $id)] | length == 0' > /dev/null 2>&1; then
    pass "Multi-tenant isolation: tenant-B decision not visible in tenant-A graph"
  else
    fail "Multi-tenant isolation: tenant-B decision leaked into tenant-A graph"
  fi
else
  fail "Multi-tenant isolation: could not capture tenant-B decision"
fi

# ── quality scan via MCP HTTP ─────────────────────────────────────────────────
section "Quality scan — MCP HTTP"

# Uses the same HTTP MCP endpoint; scans decisions already captured on the live server.
qs_resp=$(curl -s \
  -H "Content-Type: application/json" \
  -H "X-HiveMind-Tenant: $TENANT" \
  -H "X-HiveMind-Actor: agent:e2e:smoke" \
  "${mcp_auth_args[@]}" \
  "${mcp_session_args[@]}" \
  -X POST \
  -d '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"scan_decision_quality","arguments":{}}}' \
  "$BASE_URL/mcp")
if echo "$qs_resp" | jq -e '.result.content[0].text' > /dev/null 2>&1; then
  pass "MCP HTTP scan_decision_quality — returned results"
else
  fail "MCP HTTP scan_decision_quality — response: $qs_resp"
fi

# ── quality score + summarize (rule-based, always runs) ──────────────────────
section "Quality score + summarize"

if [[ -n "$DECISION_ID" ]]; then
  score_resp=$(curl -s \
    -H "Content-Type: application/json" \
    -H "X-HiveMind-Tenant: $TENANT" \
    -H "X-HiveMind-Actor: agent:e2e:smoke" \
    "${mcp_auth_args[@]}" \
    "${mcp_session_args[@]}" \
    -X POST \
    -d "{\"jsonrpc\":\"2.0\",\"id\":10,\"method\":\"tools/call\",\"params\":{\"name\":\"score_decision\",\"arguments\":{\"decision_id\":\"$DECISION_ID\"}}}" \
    "$BASE_URL/mcp")
  if echo "$score_resp" | jq -e '.result.content[0].text' > /dev/null 2>&1; then
    score_text=$(echo "$score_resp" | jq -r '.result.content[0].text')
    if echo "$score_text" | jq -e '.data | (has("framing") and has("provenance"))' > /dev/null 2>&1; then
      info_status=$(echo "$score_text" | jq -r '.data.information.status')
      info_level=$(echo "$score_text" | jq -r '.data.information.level // "not assessed"')
      pass "MCP score_decision — profile returned (information: $info_status, $info_level)"
    else
      fail "MCP score_decision — not a quality profile: $score_text"
    fi
  else
    fail "MCP score_decision — unexpected response: $score_resp"
  fi

  sum_resp=$(curl -s \
    -H "Content-Type: application/json" \
    -H "X-HiveMind-Tenant: $TENANT" \
    -H "X-HiveMind-Actor: agent:e2e:smoke" \
    "${mcp_auth_args[@]}" \
    "${mcp_session_args[@]}" \
    -X POST \
    -d "{\"jsonrpc\":\"2.0\",\"id\":11,\"method\":\"tools/call\",\"params\":{\"name\":\"summarize_decisions\",\"arguments\":{\"decision_ids\":[\"$DECISION_ID\"],\"mode\":\"single\"}}}" \
    "$BASE_URL/mcp")
  if echo "$sum_resp" | jq -e '.result.content[0].text' > /dev/null 2>&1; then
    pass "MCP summarize_decisions (single) — returned summary"
  else
    fail "MCP summarize_decisions — unexpected response: $sum_resp"
  fi
else
  skip "MCP score_decision — no decision_id (capture failed)"
  skip "MCP summarize_decisions — no decision_id (capture failed)"
fi

# ── fidelity binary — ceiling mode (always runs, no LLM) ─────────────────────
section "Fidelity binary (ceiling mode)"

if command -v "$FIDELITY_BIN" > /dev/null 2>&1 && [[ -f "$FIDELITY_CORPUS" ]]; then
  fidelity_out=$("$FIDELITY_BIN" --ceiling --corpus "$FIDELITY_CORPUS" 2>&1) || fidelity_rc=$?
  fidelity_rc="${fidelity_rc:-0}"
  if echo "$fidelity_out" | grep -qi "Macro-F1"; then
    macro_f1=$(echo "$fidelity_out" | grep -i "Macro-F1" | awk '{print $NF}')
    pass "fidelity-eval --ceiling — Macro-F1=$macro_f1 (schema ceiling, no LLM)"
  elif [[ "$fidelity_rc" -eq 0 ]]; then
    pass "fidelity-eval --ceiling — exited 0 (binary + projector smoke OK)"
  else
    fail "fidelity-eval --ceiling — exit $fidelity_rc; output: $fidelity_out"
  fi
else
  if ! command -v "$FIDELITY_BIN" > /dev/null 2>&1; then
    skip "fidelity-eval --ceiling — binary not found (FIDELITY_BIN=$FIDELITY_BIN; build: cargo build --bin fidelity-eval)"
  else
    skip "fidelity-eval --ceiling — corpus not found ($FIDELITY_CORPUS)"
  fi
fi

# ── LLM-gated (subscription/edge path) ───────────────────────────────────────
# RE-SCOPED 2026-09-07 (hivemind-fabq.3, subscription-first — no ANTHROPIC_API_KEY).
# These legs exercise the path a real coding agent actually uses: decision
# extraction happens IN-SESSION, at the edge, via the hivemind-capture plugin's
# "Batch Capture via Haiku Subagent (Keyless)" workflow (see
# plugins/hivemind-capture/skills/hivemind-capture/SKILL.md), riding the
# operator's own Claude subscription through a headless `claude -p` session —
# never a HiveMind-held ANTHROPIC_API_KEY (hivemind-mfc7). This is a DIFFERENT
# mechanism from the server-side classifier/scorer background workers
# (src/classifier.rs, src/scorer.rs), which stay dark and unexercised here.
#
# LOCAL-only by design: CI runners have no Claude subscription, so `claude` is
# absent/unauthenticated there and every leg below SKIPs cleanly — keyless CI
# stays green. Run this locally, logged in via `claude auth login`, to
# exercise the real path.
section "LLM-gated (subscription/edge path via claude -p)"

CLAUDE_BIN="${CLAUDE_BIN:-claude}"
claude_subscription_available() {
  command -v "$CLAUDE_BIN" > /dev/null 2>&1 || return 1
  "$CLAUDE_BIN" auth status --json 2>/dev/null | jq -e '.loggedIn == true' > /dev/null 2>&1
}

if [[ -n "${ANTHROPIC_API_KEY:-}" ]]; then
  skip "LLM classifier enrichment (edge) — ANTHROPIC_API_KEY is set; this leg tests the KEYLESS subscription path only — unset it to run"
  skip "LLM quality-score enrichment (edge) — ANTHROPIC_API_KEY is set"
  skip "LLM fidelity smoke (edge/claude-cli) — ANTHROPIC_API_KEY is set"
elif ! claude_subscription_available; then
  skip "LLM classifier enrichment (edge) — claude CLI not on PATH or not authenticated (LOCAL-only leg; set CLAUDE_BIN or run 'claude auth login')"
  skip "LLM quality-score enrichment (edge) — claude CLI not on PATH or not authenticated"
  skip "LLM fidelity smoke (edge/claude-cli) — claude CLI not on PATH or not authenticated"
else
  # Proxy check, not a live log inspection: ANTHROPIC_API_KEY is the same var
  # docker-compose.yml forwards into the server container. This shell has it
  # unset (the elif above would have skipped otherwise), which is consistent
  # with the server's classifier/scorer workers logging themselves disabled
  # at startup. Corroborate directly with:
  #   docker compose logs hivemind | grep 'Layer-3 classifier disabled'
  pass "server-side classifier/scorer dark: ANTHROPIC_API_KEY absent from this shell (proxy check — see comment for the direct log corroboration)"

  # LEG 1: classifier enrichment via the plugin's edge/Haiku-subagent batch
  # capture workflow, run through a REAL headless claude -p session under the
  # operator's subscription. The classifier prompt is extracted verbatim from
  # SKILL.md (single source of truth) rather than duplicated here.
  EDGE_PLUGIN_DIR="$REPO_ROOT/plugins/hivemind-capture"
  EDGE_SKILL_FILE="$EDGE_PLUGIN_DIR/skills/hivemind-capture/SKILL.md"
  EDGE_DIR=$(mktemp -d)
  EDGE_SESSION="e2e-fabq3-edge-classifier"
  EDGE_ACTOR="agent:claude:$EDGE_SESSION"
  EDGE_CAPTURES_FILE="$EDGE_DIR/captures.json"

  EDGE_PROMPT_TEMPLATE=$(awk '
    /You are the HiveMind capture classifier\.$/ { capture=1 }
    capture { print }
    /Return only the JSON array, no other text\.$/ { exit }
  ' "$EDGE_SKILL_FILE" | sed 's/^   //')

  if [[ -z "$EDGE_PROMPT_TEMPLATE" ]]; then
    fail "LLM classifier enrichment (edge) — could not extract the classifier prompt template from $EDGE_SKILL_FILE (schema drift? update the awk sentinels)"
  else
    # Fixed 4-turn transcript with one unambiguous decision moment.
    EDGE_TRANSCRIPT=$(cat <<'TRANSCRIPT'
[user] We need to pick a storage engine for the new event ledger service.
[assistant] Let's compare SQLite and Postgres for this. SQLite is simpler to
operate for a single-node prototype but does not support concurrent
multi-tenant writes well. Postgres adds an operational dependency but scales
past a single writer.
[user] We're expecting multiple agents writing concurrently from day one, so
go with whichever handles that best.
[assistant] Decided: use Postgres for the shared event ledger. Rationale:
concurrent multi-tenant writes are a day-one requirement and SQLite's
single-writer model would bottleneck immediately; Postgres's MVCC handles
concurrent writers without an external queue. Considered sqlite and postgres;
chose postgres.
TRANSCRIPT
)
    EDGE_PROMPT="$EDGE_PROMPT_TEMPLATE

Return ONLY the JSON array — no prose, no markdown code fences, no other text.

---BATCH---
$EDGE_TRANSCRIPT"

    edge_rc=0
    edge_result=$(env -u ANTHROPIC_API_KEY "$CLAUDE_BIN" -p \
      --model claude-haiku-4-5-20251001 \
      --plugin-dir "$EDGE_PLUGIN_DIR" \
      --tools "" \
      --output-format json \
      --no-session-persistence \
      --max-budget-usd 1.00 \
      "$EDGE_PROMPT" 2>&1) || edge_rc=$?

    if [[ "$edge_rc" -ne 0 ]]; then
      fail "LLM classifier enrichment (edge) — claude -p exited $edge_rc: $edge_result"
    elif ! echo "$edge_result" | jq -e '.is_error == false' > /dev/null 2>&1; then
      fail "LLM classifier enrichment (edge) — claude -p reported an error: $edge_result"
    else
      # Strip a markdown code fence if the model added one despite instructions.
      edge_json=$(echo "$edge_result" | jq -r '.result' | sed -e '/^```/d')
      if ! echo "$edge_json" | jq -e 'type == "array"' > /dev/null 2>&1; then
        fail "LLM classifier enrichment (edge) — claude -p did not return a JSON array: $edge_json"
      else
        echo "$edge_json" > "$EDGE_CAPTURES_FILE"
        edge_emit=$("$HIVEMIND_BIN" --hivemind-dir "$EDGE_DIR" --actor "$EDGE_ACTOR" --json \
          emit ingest.batch_classified \
          --captures "$EDGE_CAPTURES_FILE" \
          --agent-tool claude \
          --agent-session "$EDGE_SESSION" \
          --classifier-model claude-haiku-4-5-20251001 2>&1) || true
        if echo "$edge_emit" | jq -e '.kind == "batch_id"' > /dev/null 2>&1; then
          EDGE_BATCH_ID=$(echo "$edge_emit" | jq -r '.value')
          pass "LLM classifier enrichment (edge): claude -p classified the transcript, batch $EDGE_BATCH_ID submitted via ingest.batch_classified"

          edge_activity=$("$HIVEMIND_BIN" --hivemind-dir "$EDGE_DIR" --json query get_recent_activity --actor-id "$EDGE_ACTOR" --limit 10)
          if echo "$edge_activity" | jq -e '.data.items[] | select(.event_type == "ingest.batch_classified")' > /dev/null 2>&1; then
            pass "LLM classifier enrichment (edge): ingest.batch_classified event lands in the ledger (schema parity with src/classifier.rs)"
          else
            fail "LLM classifier enrichment (edge): batch_id returned but no ingest.batch_classified event found — response: $edge_activity"
          fi
        else
          fail "LLM classifier enrichment (edge): claude -p returned valid captures but ingest.batch_classified emit failed: $edge_emit — captures: $edge_json"
        fi
      fi
    fi
  fi

  # LEG 2: quality-score enrichment via the same edge shape, scoring the
  # decision capture LEG 1 just submitted, through the plugin's "Batch Score
  # via Haiku Subagent (Keyless)" workflow (SKILL.md). The scorer prompt is
  # extracted verbatim from SKILL.md (single source of truth) rather than
  # duplicated here — same pattern as LEG 1's classifier prompt extraction.
  if [[ -z "${EDGE_BATCH_ID:-}" ]]; then
    skip "LLM quality-score enrichment (edge) — LEG 1 did not produce a batch_id to score"
  else
    EDGE_DECISION_IDX=$(echo "$edge_json" | jq 'map(.kind == "decision") | index(true)')
    if [[ "$EDGE_DECISION_IDX" == "null" || -z "$EDGE_DECISION_IDX" ]]; then
      fail "LLM quality-score enrichment (edge) — no decision-kind capture in LEG 1's batch to score"
    else
      EDGE_SCORE_PROMPT_TEMPLATE=$(awk '
        /You are the HiveMind decision scorer\.$/ { capture=1 }
        /^   ---DECISION---$/ { exit }
        capture { print }
      ' "$EDGE_SKILL_FILE" | sed 's/^   //')

      if [[ -z "$EDGE_SCORE_PROMPT_TEMPLATE" ]]; then
        fail "LLM quality-score enrichment (edge) — could not extract the scorer prompt template from $EDGE_SKILL_FILE (schema drift? update the awk sentinels)"
      else
        EDGE_DECISION_CAPTURE=$(echo "$edge_json" | jq ".[$EDGE_DECISION_IDX]")
        EDGE_DECISION_TEXT=$(echo "$EDGE_DECISION_CAPTURE" | jq -r '
          "Title: " + .title
          + "\nRationale: " + .rationale
          + "\nOptions considered: " + ((.options // []) | join(", "))
          + "\nChosen option: " + (.chosen_option // "none")
          + "\nExpressed confidence: " + (.expressed_confidence // "unstated")
        ')

        EDGE_SCORE_PROMPT="$EDGE_SCORE_PROMPT_TEMPLATE

---DECISION---
$EDGE_DECISION_TEXT"

        # claude -p's raw JSON output occasionally has a discipline slip on
        # this leg — a duplicate key (jq's has() check below is permissive
        # and last-value-wins, so it slips past the schema guard and is only
        # caught by decision.scored's strict serde_json parser at emit time)
        # or a missing comma (fails the schema guard outright because the
        # text isn't valid JSON at all). Both shapes have been seen against a
        # live stack, varying run to run. This leg exercises the keyless
        # subscription transport path, not the model's raw JSON discipline,
        # so one retry on either content-shape miss distinguishes that from
        # a real transport/CLI defect (nonzero exit, reported error) before
        # we call it a failure.
        edge_score_attempt() {
          edge_score_outcome=""
          edge_score_rc=0
          edge_score_result=$(env -u ANTHROPIC_API_KEY "$CLAUDE_BIN" -p \
            --model claude-haiku-4-5-20251001 \
            --plugin-dir "$EDGE_PLUGIN_DIR" \
            --tools "" \
            --output-format json \
            --no-session-persistence \
            --max-budget-usd 1.00 \
            "$EDGE_SCORE_PROMPT" 2>&1) || edge_score_rc=$?

          if [[ "$edge_score_rc" -ne 0 ]]; then
            edge_score_outcome="call_fail"
            return
          fi
          if ! echo "$edge_score_result" | jq -e '.is_error == false' > /dev/null 2>&1; then
            edge_score_outcome="call_fail"
            return
          fi

          # Strip a markdown code fence if the model added one despite instructions.
          edge_score_json=$(echo "$edge_score_result" | jq -r '.result' | sed -e '/^```/d')
          if ! echo "$edge_score_json" | jq -e 'has("quality_dims") and has("importance")' > /dev/null 2>&1; then
            edge_score_outcome="schema_fail"
            return
          fi

          EDGE_SCORES_FILE="$EDGE_DIR/scores.json"
          echo "$edge_score_json" > "$EDGE_SCORES_FILE"
          edge_score_emit=$("$HIVEMIND_BIN" --hivemind-dir "$EDGE_DIR" --actor "$EDGE_ACTOR" --json \
            emit decision.scored \
            --batch-id "$EDGE_BATCH_ID" \
            --capture-index "$EDGE_DECISION_IDX" \
            --scores "$EDGE_SCORES_FILE" \
            --agent-tool claude \
            --agent-session "$EDGE_SESSION" \
            --scorer-model claude-haiku-4-5-20251001 2>&1) || true
          if echo "$edge_score_emit" | jq -e '.kind == "event_id"' > /dev/null 2>&1; then
            edge_score_outcome="ok"
          else
            edge_score_outcome="emit_fail"
          fi
        }

        edge_score_attempt

        if [[ "$edge_score_outcome" == "schema_fail" || "$edge_score_outcome" == "emit_fail" ]]; then
          edge_score_attempt
        fi

        case "$edge_score_outcome" in
          ok)
            edge_score_event_id=$(echo "$edge_score_emit" | jq -r '.value')
            pass "LLM quality-score enrichment (edge): claude -p scored the decision, event $edge_score_event_id submitted via decision.scored"

            edge_score_activity=$("$HIVEMIND_BIN" --hivemind-dir "$EDGE_DIR" --json query get_recent_activity --actor-id "$EDGE_ACTOR" --limit 10)
            if echo "$edge_score_activity" | jq -e '.data.items[] | select(.event_type == "decision.scored")' > /dev/null 2>&1; then
              pass "LLM quality-score enrichment (edge): decision.scored event lands in the ledger (schema parity with src/scorer.rs)"
            else
              fail "LLM quality-score enrichment (edge): event_id returned but no decision.scored event found — response: $edge_score_activity"
            fi
            ;;
          call_fail)
            if [[ "$edge_score_rc" -ne 0 ]]; then
              fail "LLM quality-score enrichment (edge) — claude -p exited $edge_score_rc: $edge_score_result"
            else
              fail "LLM quality-score enrichment (edge) — claude -p reported an error: $edge_score_result"
            fi
            ;;
          schema_fail)
            skip "LLM quality-score enrichment (edge) — claude -p did not return the expected scores schema after a retry (model JSON-discipline nondeterminism, not a code defect): $edge_score_json"
            ;;
          emit_fail)
            skip "LLM quality-score enrichment (edge) — claude -p returned schema-shaped but strictly-invalid JSON rejected by decision.scored after a retry (model JSON-discipline nondeterminism, not a code defect): $edge_score_emit — scores: $edge_score_json"
            ;;
        esac
      fi
    fi
  fi

  # LEG 3: fidelity smoke via the evaluator's claude-cli/subscription backend
  # (hivemind-265w). fidelity-eval auto-selects the keyless claude-cli backend
  # when ANTHROPIC_API_KEY is unset and `claude` is on PATH — same resolution
  # LEG 1/2 rely on. Runs the real Haiku classifier on the 2-case smoke corpus
  # (not the full 39-case corpus) to prove the pipeline is wired end-to-end.
  if ! command -v "$FIDELITY_BIN" > /dev/null 2>&1; then
    skip "LLM fidelity smoke (edge/claude-cli) — fidelity-eval binary not found (FIDELITY_BIN=$FIDELITY_BIN; build: cargo build --bin fidelity-eval)"
  elif [[ ! -f "$FIDELITY_CORPUS_SMOKE" ]]; then
    skip "LLM fidelity smoke (edge/claude-cli) — smoke corpus not found ($FIDELITY_CORPUS_SMOKE)"
  else
    fidelity_smoke_rc=0
    fidelity_smoke_out=$(env -u ANTHROPIC_API_KEY "$FIDELITY_BIN" --corpus "$FIDELITY_CORPUS_SMOKE" 2>&1) || fidelity_smoke_rc=$?
    if [[ "$fidelity_smoke_rc" -ne 0 ]]; then
      fail "LLM fidelity smoke (edge/claude-cli) — fidelity-eval exited $fidelity_smoke_rc: $fidelity_smoke_out"
    elif ! echo "$fidelity_smoke_out" | grep -q "Backend: claude-cli"; then
      fail "LLM fidelity smoke (edge/claude-cli) — fidelity-eval did not select the claude-cli backend: $fidelity_smoke_out"
    elif echo "$fidelity_smoke_out" | grep -q "classifier error for"; then
      fail "LLM fidelity smoke (edge/claude-cli) — claude -p classifier call failed for a smoke case: $(echo "$fidelity_smoke_out" | grep "classifier error for")"
    elif ! echo "$fidelity_smoke_out" | grep -q "Macro-F1"; then
      fail "LLM fidelity smoke (edge/claude-cli) — no Macro-F1 headline in fidelity-eval output: $fidelity_smoke_out"
    else
      fidelity_smoke_f1=$(echo "$fidelity_smoke_out" | grep "Macro-F1" | awk '{print $NF}')
      pass "LLM fidelity smoke (edge/claude-cli): claude-cli backend ran both smoke cases via claude -p, Macro-F1=$fidelity_smoke_f1"
    fi
  fi

  # LEG 4: hivemind-context plugin slash-command smoke (hivemind-tenv.3
  # reopen). Drives the plugin through a REAL `claude -p` session issuing the
  # actual slash command with an UNQUOTED multi-word free-text description —
  # the natural, unprompted phrasing an agent reaches for first, and the
  # exact shape that broke before this leg existed. Calling
  # plugins/hivemind-context/scripts/recall.sh directly cannot catch this
  # class of bug: the defect was in how Claude Code's $ARGUMENTS
  # command-template substitution fed the script, not in the script's own
  # argument handling in isolation — that gap is why the original landing
  # (PR #33) shipped it and only a real plugin invocation during tenv.5's
  # fan-in re-verification found it.
  if ! command -v "$HIVEMIND_BIN" > /dev/null 2>&1; then
    skip "hivemind-context plugin slash-command smoke — hivemind binary not found (HIVEMIND_BIN=$HIVEMIND_BIN)"
  else
    CONTEXT_PLUGIN_DIR="$REPO_ROOT/plugins/hivemind-context"
    CONTEXT_DIR=$(mktemp -d)
    CONTEXT_TITLE="Plugin smoke test fixture decision"

    context_seed_rc=0
    "$HIVEMIND_BIN" --hivemind-dir "$CONTEXT_DIR" --actor "agent:e2e:smoke-context" --json \
      emit decision.proposed \
      --title "$CONTEXT_TITLE" \
      --rationale "Seeded by scripts/e2e_smoke.sh LEG 4 so a real /hivemind-context:recall slash-command invocation has something to match." \
      --options fixture \
      --chose fixture \
      --topic-keys plugin-smoke-fixture \
      > /dev/null 2>&1 || context_seed_rc=$?

    if [[ "$context_seed_rc" -ne 0 ]]; then
      fail "hivemind-context plugin slash-command smoke — could not seed the fixture decision (exit $context_seed_rc)"
    else
      # Bash-tool subprocesses spawned by claude -p do not inherit an
      # arbitrary HIVEMIND_DIR env var reliably (verified empirically: it
      # fell back to the plugin's default ./hivemind/ resolution). Point the
      # plugin at the fixture ledger the documented way instead — a trailing
      # --hivemind-dir flag, which the plugin's scripts forward straight to
      # the CLI (README: "or pass --hivemind-dir to the underlying
      # scripts"). This also exercises the fix for the multi-word
      # description + trailing flag case, not just the bare description
      # case. HIVEMIND_CAPTURE_BIN *does* propagate (verified) and pins the
      # binary under test rather than whatever `hivemind` is on PATH.
      context_prompt="/hivemind-context:recall plugin smoke test fixture decision --hivemind-dir $CONTEXT_DIR"
      context_rc=0
      # stream-json + --verbose, not the single-result "json" format: a
      # Haiku session that hits the bug on its first Bash call reliably
      # self-corrects by re-invoking with manual quotes on the next turn
      # (observed directly while building this leg), so the FINAL result
      # text looks clean even when the regression fired. Only the raw
      # tool_result stream shows the first attempt honestly.
      context_result=$(env -u ANTHROPIC_API_KEY HIVEMIND_CAPTURE_BIN="$HIVEMIND_BIN" \
        "$CLAUDE_BIN" -p "$context_prompt" \
        --model claude-haiku-4-5-20251001 \
        --plugin-dir "$CONTEXT_PLUGIN_DIR" \
        --output-format stream-json \
        --verbose \
        --no-session-persistence \
        --permission-mode bypassPermissions \
        --max-budget-usd 1.00 < /dev/null 2>&1) || context_rc=$?

      if [[ "$context_rc" -ne 0 ]]; then
        fail "hivemind-context plugin slash-command smoke — claude -p exited $context_rc: $context_result"
      else
        context_tool_results=$(echo "$context_result" | jq -c 'select(.type=="user") | .message.content[]? | select(.type=="tool_result")' 2>/dev/null)
        if [[ -z "$context_tool_results" ]]; then
          fail "hivemind-context plugin slash-command smoke — no Bash tool_result observed in the transcript: $context_result"
        elif echo "$context_tool_results" | grep -qi "unexpected argument"; then
          fail "hivemind-context plugin slash-command smoke — a Bash tool_result shows the CLI rejected the unquoted multi-word description (the exact hivemind-tenv.3 reopen regression), even if a later retry in the same session masked it: $context_tool_results"
        elif echo "$context_tool_results" | grep -qF "$CONTEXT_TITLE"; then
          pass "hivemind-context plugin slash-command smoke: unquoted multi-word /hivemind-context:recall round-tripped through claude -p and matched the seeded fixture decision, on the first attempt"
        else
          fail "hivemind-context plugin slash-command smoke — ran without the arg-parsing regression but never surfaced the seeded fixture decision: $context_tool_results"
        fi
      fi
    fi
  fi
fi

# ── 401 regression (auth-mode only) ──────────────────────────────────────────
section "401 regression"

if [[ -n "$API_KEY" ]]; then
  unauth_status=$(curl -s -o /dev/null -w "%{http_code}" \
    -X POST "$BASE_URL/v1/decisions" \
    -H "Content-Type: application/json" \
    -H "X-HiveMind-Tenant: $TENANT" \
    -H "X-HiveMind-Actor: agent:e2e:smoke" \
    -d '{"title":"401-regression","rationale":"test","topic_keys":[],"options":[{"label":"x"}]}')
  if [[ "$unauth_status" == "401" ]]; then
    pass "POST /v1/decisions without bearer → 401 (server enforces auth)"
  else
    fail "POST /v1/decisions without bearer → expected 401, got $unauth_status"
  fi
else
  skip "401 regression — no-auth mode (dev/SQLite without API_KEY)"
fi

# ── summary ───────────────────────────────────────────────────────────────────
echo
echo "────────────────────────────────────────"
echo "Results: PASS=$PASS  FAIL=$FAIL  SKIP=$SKIP"
echo "────────────────────────────────────────"
echo

if [[ $FAIL -gt 0 ]]; then
  echo "SMOKE TEST FAILED ($FAIL failure(s))" >&2
  exit 1
fi
echo "SMOKE TEST PASSED"
exit 0
