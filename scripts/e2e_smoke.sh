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
#   ANTHROPIC_API_KEY       — when set, LLM-gated assertions are enabled
#   HIVEMIND_BIN            — path to hivemind binary (for CLI + MCP legs)
#
# Exit codes: 0 = all checks passed; 1 = at least one check failed.
#
# Minimal runtime deps: curl, jq.  hivemind binary for CLI/MCP legs.
# The script does NOT start or stop the server — call it after compose is up.

set -euo pipefail

# ── defaults ──────────────────────────────────────────────────────────────────
BASE_URL="${HIVEMIND_E2E_BASE_URL:-http://localhost:8080}"
API_KEY="${HIVEMIND_E2E_API_KEY:-}"
TENANT="${HIVEMIND_E2E_TENANT:-e2e-test}"
HIVEMIND_BIN="${HIVEMIND_BIN:-hivemind}"
SKIP_MAP="${HIVEMIND_E2E_SKIP_MAP:-false}"   # set true for Postgres (270r)

# ── arg parsing ───────────────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
  case "$1" in
    --base-url)  BASE_URL="$2";  shift 2 ;;
    --api-key)   API_KEY="$2";   shift 2 ;;
    --tenant)    TENANT="$2";    shift 2 ;;
    --skip-map)  SKIP_MAP=true;  shift   ;;
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
  curl -sf \
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
  curl -sf \
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
trap 'rm -rf "$CLI_DATA_DIR"' EXIT

if command -v "$HIVEMIND_BIN" > /dev/null 2>&1; then
  cli_out=$("$HIVEMIND_BIN" \
    --hivemind-dir "$CLI_DATA_DIR" \
    --actor "agent:e2e:smoke-cli" \
    --tenant "$TENANT" \
    --json \
    emit decision.proposed \
    --title "e2e-smoke CLI: adopt semantic versioning" \
    --rationale "Semver gives downstream consumers predictable upgrade signals" 2>&1) || true
  if echo "$cli_out" | jq -e '.decision_id' > /dev/null 2>&1; then
    CLI_DECISION_ID=$(echo "$cli_out" | jq -r '.decision_id')
    pass "CLI emit decision.proposed — $CLI_DECISION_ID"
  else
    fail "CLI emit decision.proposed — output: $cli_out"
  fi
else
  skip "CLI leg — hivemind binary not on PATH (set HIVEMIND_BIN)"
fi

# ── capture via MCP stdio ─────────────────────────────────────────────────────
section "Capture — MCP stdio"

MCP_DATA_DIR=$(mktemp -d)
trap 'rm -rf "$CLI_DATA_DIR" "$MCP_DATA_DIR"' EXIT

if command -v "$HIVEMIND_BIN" > /dev/null 2>&1; then
  MCP_SESSION_ID="e2e-smoke-mcp-$(date +%s)"
  mcp_capture=$(printf '%s\n%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize"}' \
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"capture_decision","arguments":{"actor_id":"agent:e2e:smoke-mcp","title":"e2e-smoke MCP: prefer immutable events","rationale":"Immutable append-only log simplifies auditing"}}}' \
    | "$HIVEMIND_BIN" \
        --hivemind-dir "$MCP_DATA_DIR" \
        --actor "agent:e2e:smoke-mcp" \
        --tenant "$TENANT" \
        mcp --session-id "$MCP_SESSION_ID" 2>/dev/null \
    | tail -1)
  if echo "$mcp_capture" | jq -e '.result.content[0].text' > /dev/null 2>&1; then
    mcp_text=$(echo "$mcp_capture" | jq -r '.result.content[0].text')
    if echo "$mcp_text" | jq -e '.decision_id' > /dev/null 2>&1; then
      MCP_DECISION_ID=$(echo "$mcp_text" | jq -r '.decision_id')
      pass "MCP capture_decision — $MCP_DECISION_ID"
    else
      fail "MCP capture_decision — tool text: $mcp_text"
    fi
  else
    fail "MCP capture_decision — unexpected response: $mcp_capture"
  fi
else
  skip "MCP leg — hivemind binary not on PATH (set HIVEMIND_BIN)"
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

search_resp=$(curl_api GET "/v1/decisions/search?q=postgres")
if echo "$search_resp" | jq -e 'has("data")' > /dev/null 2>&1; then
  count=$(echo "$search_resp" | jq '.data.total_matches // (.data.items | length) // 0')
  pass "GET /v1/decisions/search?q=postgres — $count result(s)"
else
  fail "GET /v1/decisions/search — response: $search_resp"
fi

relevant_resp=$(curl_api GET "/v1/decisions/relevant?topics=storage")
if echo "$relevant_resp" | jq -e 'has("data")' > /dev/null 2>&1; then
  pass "GET /v1/decisions/relevant?topics=storage"
else
  fail "GET /v1/decisions/relevant — response: $relevant_resp"
fi

# ── spectral map ──────────────────────────────────────────────────────────────
section "Spectral map"

if [[ "$SKIP_MAP" == "true" ]]; then
  skip "GET /v1/decisions/map — skipped on Postgres backend (hivemind-270r: SQLite only)"
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
  [[ -n "$API_KEY" ]] && auth_args+=(-H "Authorization: Bearer $API_KEY")
  curl -sf \
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

# ── quality scan via MCP ──────────────────────────────────────────────────────
section "Quality scan — MCP"

if command -v "$HIVEMIND_BIN" > /dev/null 2>&1; then
  # Seed a few decisions first so the scanner has data
  QS_DATA_DIR=$(mktemp -d)
  trap 'rm -rf "$CLI_DATA_DIR" "$MCP_DATA_DIR" "$QS_DATA_DIR"' EXIT
  for i in 1 2 3; do
    "$HIVEMIND_BIN" \
      --hivemind-dir "$QS_DATA_DIR" \
      --actor "agent:e2e:smoke-qs" \
      --tenant "qs-test" \
      --json \
      emit decision.proposed \
      --title "e2e qs decision $i" \
      --rationale "quality scan smoke $i" > /dev/null 2>&1 || true
  done

  QS_SESSION="e2e-qs-$(date +%s)"
  qs_resp=$(printf '%s\n%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize"}' \
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"scan_decision_quality","arguments":{"actor_id":"agent:e2e:smoke-qs"}}}' \
    | "$HIVEMIND_BIN" \
        --hivemind-dir "$QS_DATA_DIR" \
        --actor "agent:e2e:smoke-qs" \
        --tenant "qs-test" \
        mcp --session-id "$QS_SESSION" 2>/dev/null \
    | tail -1)
  if echo "$qs_resp" | jq -e '.result.content[0].text' > /dev/null 2>&1; then
    pass "MCP scan_decision_quality — returned results"
  else
    fail "MCP scan_decision_quality — response: $qs_resp"
  fi
else
  skip "MCP quality scan leg — hivemind binary not on PATH"
fi

# ── LLM-gated assertions ──────────────────────────────────────────────────────
section "LLM-gated (ANTHROPIC_API_KEY)"

if [[ -z "${ANTHROPIC_API_KEY:-}" ]]; then
  skip "LLM classifier enrichment — ANTHROPIC_API_KEY not set (keyless CI, skipping)"
  skip "LLM summarize — ANTHROPIC_API_KEY not set"
else
  # When the key is set, the classifier runs asynchronously on ingest.
  # We can only verify the server accepted a decision without error;
  # classifier output is non-deterministic timing.
  llm_resp=$(curl_json POST /v1/decisions '{
    "title": "e2e-smoke: LLM classifier check",
    "rationale": "verify classifier is wired when ANTHROPIC_API_KEY is present",
    "topic_keys": ["e2e", "llm"]
  }')
  if echo "$llm_resp" | jq -e '.decision_id' > /dev/null 2>&1; then
    pass "LLM path: capture accepted with ANTHROPIC_API_KEY set (classifier may be enriching)"
  else
    fail "LLM path: capture failed with key set — response: $llm_resp"
  fi
  skip "LLM summarize — deterministic assertion not implemented in Slice 1 (Slice 2)"
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
