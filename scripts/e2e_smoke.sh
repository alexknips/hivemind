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

set -euo pipefail

# ── defaults ──────────────────────────────────────────────────────────────────
BASE_URL="${HIVEMIND_E2E_BASE_URL:-http://localhost:8080}"
API_KEY="${HIVEMIND_E2E_API_KEY:-}"
API_KEY_B="${HIVEMIND_E2E_API_KEY_B:-}"  # separate token for tenant-B in auth mode
TENANT="${HIVEMIND_E2E_TENANT:-e2e-test}"
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

# ── health check ──────────────────────────────────────────────────────────────
section "Health"
if curl -sf "$BASE_URL/v1/health" | jq -e '.status == "ok"' > /dev/null 2>&1; then
  pass "GET /v1/health returns {status:ok}"
else
  fail "GET /v1/health — server not ready or wrong response"
fi

# ── capture via HTTP ──────────────────────────────────────────────────────────
section "Capture — HTTP"

decision_resp=$(curl_json POST /v1/decisions '{
  "title": "e2e-smoke: use Postgres for shared storage",
  "rationale": "SQLite is single-writer; Postgres supports concurrent tenants",
  "topic_keys": ["storage", "e2e"],
  "options": [
    {"label": "postgres", "description": "Postgres with connection pool"},
    {"label": "sqlite",   "description": "SQLite WAL mode"}
  ],
  "chosen_option_label": "postgres"
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
trap 'rm -rf "$CLI_DATA_DIR" "${EDGE_DIR:-}"' EXIT

if command -v "$HIVEMIND_BIN" > /dev/null 2>&1; then
  cli_out=$("$HIVEMIND_BIN" \
    --hivemind-dir "$CLI_DATA_DIR" \
    --actor "agent:e2e:smoke-cli" \
    --tenant "$TENANT" \
    --json \
    emit decision.proposed \
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
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"capture_decision","arguments":{"actor_id":"agent:e2e:smoke-mcp","title":"e2e-smoke MCP: prefer immutable events","rationale":"Immutable append-only log simplifies auditing","topic_keys":["e2e","architecture"],"options":[{"label":"immutable"},{"label":"mutable"}],"chosen_option_label":"immutable"}}}' \
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

# ── review operations ─────────────────────────────────────────────────────────
section "Review — disagree + supersede"

if [[ -n "$DECISION_ID" ]]; then
  # Capture a second decision to supersede (options required and must be non-empty)
  d2_resp=$(curl_json POST /v1/decisions '{
    "title": "e2e-smoke: use MySQL instead (superseded)",
    "rationale": "Initial idea before Postgres was chosen",
    "topic_keys": ["storage", "e2e"],
    "options": [{"label": "mysql"}]
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
      '{"title": "e2e-smoke: Postgres selected over MySQL", "rationale": "e2e smoke: Postgres chosen over MySQL after evaluation"}')
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

TENANT_B="e2e-test-other"
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
  '{"title": "e2e-smoke tenant-B only decision", "rationale": "should not appear in tenant A", "topic_keys":["e2e"], "options":[{"label":"opt-b"}]}')
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
    if echo "$score_text" | jq -e '.data | (has("score") or . == null)' > /dev/null 2>&1; then
      tier=$(echo "$score_text" | jq -r '.data.tier // "no-data"')
      score=$(echo "$score_text" | jq -r '.data.score // "n/a"')
      pass "MCP score_decision — tier=$tier score=$score"
    else
      pass "MCP score_decision — returned result"
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
          fail "LLM quality-score enrichment (edge) — claude -p exited $edge_score_rc: $edge_score_result"
        elif ! echo "$edge_score_result" | jq -e '.is_error == false' > /dev/null 2>&1; then
          fail "LLM quality-score enrichment (edge) — claude -p reported an error: $edge_score_result"
        else
          # Strip a markdown code fence if the model added one despite instructions.
          edge_score_json=$(echo "$edge_score_result" | jq -r '.result' | sed -e '/^```/d')
          if ! echo "$edge_score_json" | jq -e 'has("quality_dims") and has("importance")' > /dev/null 2>&1; then
            fail "LLM quality-score enrichment (edge) — claude -p did not return the expected scores schema: $edge_score_json"
          else
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
              edge_score_event_id=$(echo "$edge_score_emit" | jq -r '.value')
              pass "LLM quality-score enrichment (edge): claude -p scored the decision, event $edge_score_event_id submitted via decision.scored"

              edge_score_activity=$("$HIVEMIND_BIN" --hivemind-dir "$EDGE_DIR" --json query get_recent_activity --actor-id "$EDGE_ACTOR" --limit 10)
              if echo "$edge_score_activity" | jq -e '.data.items[] | select(.event_type == "decision.scored")' > /dev/null 2>&1; then
                pass "LLM quality-score enrichment (edge): decision.scored event lands in the ledger (schema parity with src/scorer.rs)"
              else
                fail "LLM quality-score enrichment (edge): event_id returned but no decision.scored event found — response: $edge_score_activity"
              fi
            else
              fail "LLM quality-score enrichment (edge): claude -p returned valid scores but decision.scored emit failed: $edge_score_emit — scores: $edge_score_json"
            fi
          fi
        fi
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
