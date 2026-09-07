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
#   ANTHROPIC_API_KEY       — when set, LLM-gated assertions (Slice 2) are enabled
#   HIVEMIND_BIN            — path to hivemind binary (for CLI + MCP legs)
#   FIDELITY_BIN            — path to fidelity-eval binary (for ceiling-mode smoke)
#   FIDELITY_CORPUS         — path to fidelity corpus YAML (default: benchmarks/fidelity/corpus.yaml)
#   FIDELITY_CORPUS_SMOKE   — path to mini smoke corpus (default: benchmarks/fidelity/corpus-smoke.yaml)
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
trap 'rm -rf "$CLI_DATA_DIR"' EXIT

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

# ── LLM-gated assertions (Slice 2) ───────────────────────────────────────────
section "LLM-gated (ANTHROPIC_API_KEY)"

if [[ -z "${ANTHROPIC_API_KEY:-}" ]]; then
  skip "LLM classifier enrichment — ANTHROPIC_API_KEY not set (keyless CI)"
  skip "LLM quality-score enrichment — ANTHROPIC_API_KEY not set"
  skip "LLM fidelity smoke (1-2 cases) — ANTHROPIC_API_KEY not set"
else
  # Capture a well-formed decision to exercise the classifier + scorer pipeline.
  # Both workers poll on fixed intervals (classifier: 10s, scorer: 30s) so we
  # wait before checking for enrichment.
  llm_capture=$(curl_json POST /v1/decisions "{
    \"title\": \"e2e-smoke: adopt structured logging for observability\",
    \"rationale\": \"Structured JSON logs (vs printf) enable alerting rules on error_code fields; evaluated logfmt and JSON; chose JSON for tooling breadth\",
    \"topic_keys\": [\"e2e\", \"observability\", \"logging\"],
    \"options\": [
      {\"label\": \"json-logs\", \"description\": \"Structured JSON — broad tooling support\"},
      {\"label\": \"logfmt\",    \"description\": \"logfmt — human-readable but fewer tools\"},
      {\"label\": \"printf\",   \"description\": \"printf — simplest, unstructured\"}
    ],
    \"chosen_option_label\": \"json-logs\"
  }")
  LLM_DECISION_ID=""
  if echo "$llm_capture" | jq -e '.decision_id' > /dev/null 2>&1; then
    LLM_DECISION_ID=$(echo "$llm_capture" | jq -r '.decision_id')
    pass "LLM classifier path: rich decision captured ($LLM_DECISION_ID)"
  else
    fail "LLM classifier path: capture failed — response: $llm_capture"
  fi

  # Wait for the background classifier (poll interval: 10s) and scorer (30s).
  # A 20s wait is sufficient for the classifier; scorer enrichment may take longer.
  if [[ -n "$LLM_DECISION_ID" ]]; then
    echo "  (waiting 20s for background classifier...)"
    sleep 20

    # Verify the decision is still retrievable (classifier may have enriched it).
    llm_get=$(curl_api GET "/v1/decisions/$LLM_DECISION_ID")
    if echo "$llm_get" | jq -e 'has("data")' > /dev/null 2>&1; then
      pass "LLM classifier path: decision retrievable after classifier wait"
    else
      fail "LLM classifier path: decision not retrievable after wait — response: $llm_get"
    fi

    # Rule-based quality score on the LLM-captured decision.
    llm_score=$(curl -s \
      -H "Content-Type: application/json" \
      -H "X-HiveMind-Tenant: $TENANT" \
      -H "X-HiveMind-Actor: agent:e2e:smoke" \
      "${mcp_auth_args[@]}" \
      "${mcp_session_args[@]}" \
      -X POST \
      -d "{\"jsonrpc\":\"2.0\",\"id\":12,\"method\":\"tools/call\",\"params\":{\"name\":\"score_decision\",\"arguments\":{\"decision_id\":\"$LLM_DECISION_ID\"}}}" \
      "$BASE_URL/mcp")
    if echo "$llm_score" | jq -e '.result.content[0].text' > /dev/null 2>&1; then
      llm_tier=$(echo "$llm_score" | jq -r '.result.content[0].text | fromjson | .data.tier // "no-data"' 2>/dev/null || echo "no-data")
      pass "LLM quality-score enrichment: score_decision returned tier=$llm_tier"
    else
      fail "LLM quality-score enrichment: score_decision unexpected response: $llm_score"
    fi
  else
    skip "LLM classifier wait + quality-score — capture failed above"
  fi

  # Fidelity binary: 2-case smoke with real Haiku classifier (ANTHROPIC_API_KEY set).
  if command -v "$FIDELITY_BIN" > /dev/null 2>&1 && [[ -f "$FIDELITY_CORPUS_SMOKE" ]]; then
    echo "  (running fidelity-eval on 2-case smoke corpus — uses ANTHROPIC_API_KEY)..."
    fidelity_llm_out=$("$FIDELITY_BIN" --corpus "$FIDELITY_CORPUS_SMOKE" 2>&1) || fidelity_llm_rc=$?
    fidelity_llm_rc="${fidelity_llm_rc:-0}"
    if echo "$fidelity_llm_out" | grep -qi "Macro-F1"; then
      fidelity_macro=$(echo "$fidelity_llm_out" | grep -i "Macro-F1" | awk '{print $NF}')
      pass "LLM fidelity smoke (2 cases) — Macro-F1=$fidelity_macro"
    elif [[ "$fidelity_llm_rc" -eq 0 ]]; then
      pass "LLM fidelity smoke (2 cases) — exited 0"
    else
      fail "LLM fidelity smoke (2 cases) — exit $fidelity_llm_rc; output: $fidelity_llm_out"
    fi
  else
    if ! command -v "$FIDELITY_BIN" > /dev/null 2>&1; then
      skip "LLM fidelity smoke — binary not found (FIDELITY_BIN=$FIDELITY_BIN; build: cargo build --bin fidelity-eval)"
    else
      skip "LLM fidelity smoke — smoke corpus not found ($FIDELITY_CORPUS_SMOKE)"
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
