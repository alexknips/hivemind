#!/usr/bin/env bash
# load_showcase.sh — Populate a HiveMind ledger with the Acme Payments
# showcase corpus for SPA + spectral-map demonstration.
#
# Usage:
#   ./demos/showcase/load_showcase.sh [--hivemind-dir ./hivemind/] [--tenant demo]
#
# Prerequisites:
#   cargo build --release --bin hivemind  (or hivemind on PATH)
#
# What this script loads:
#   - 7 evidence nodes
#   - 3 hypothesis nodes
#   - 25 decision nodes (with options, acceptances, rejections, supersessions)
#   - 1 decision request
#   - 2 blockers (and their resolutions)
#   - 9 semantic relations (SUPPORTS, ASSUMES, REFUTES)
#
# After loading, verify with:
#   hivemind query decisions --hivemind-dir "$HIVEMIND_DIR" --tenant "$TENANT" --json
#
# Scorer note: decision.scored events require branch
#   hivemind-m2-shared-backend-lives-uuq9.19 to be merged.
#   The EMIT_SCORES variable below gates those calls.
# ------------------------------------------------------------------------------
set -euo pipefail

# ── Config ─────────────────────────────────────────────────────────────────
HIVEMIND_DIR="${HIVEMIND_DIR:-./hivemind/}"
TENANT="${TENANT:-demo}"
EMIT_SCORES="${EMIT_SCORES:-false}"   # set to "true" after scorer branch merges

# Resolve binary
if command -v hivemind &>/dev/null; then
  HM="hivemind"
elif [ -f "./target/release/hivemind" ]; then
  HM="./target/release/hivemind"
elif [ -f "./target/debug/hivemind" ]; then
  HM="./target/debug/hivemind"
else
  echo "ERROR: hivemind binary not found. Run: cargo build --bin hivemind" >&2
  exit 1
fi

HM_FLAGS="--hivemind-dir $HIVEMIND_DIR --tenant $TENANT"

hm() { $HM $HM_FLAGS "$@"; }

echo "=== Acme Payments showcase corpus loader ==="
echo "  dir:    $HIVEMIND_DIR"
echo "  tenant: $TENANT"
echo "  scores: $EMIT_SCORES"
echo ""

# ── Evidence ───────────────────────────────────────────────────────────────
echo "--- Evidence ---"

hm emit evidence.recorded \
  --actor "agent:claude:arch" \
  --evidence-id "ev-001" \
  --content "Load-test run 2026-01-08: Postgres sustained 220 TPS/tenant at 50 concurrent connections on db.t3.xlarge. MySQL reached 190 TPS but showed lock contention at 30+ connections. DynamoDB showed p99 latency > 90 ms on aggregate queries."

hm emit evidence.recorded \
  --actor "agent:claude:arch" \
  --evidence-id "ev-002" \
  --content "Stripe API integration spike: p99 charge latency 320 ms, p50 180 ms across 10k sample transactions. Adyen average 410 ms, higher variance. Square unavailable for B2B-only use-case without retail plan."

hm emit evidence.recorded \
  --actor "human:sarah-l" \
  --evidence-id "ev-003" \
  --content "Legal memo 2026-03-15: SAQ-A applies when card data is handled exclusively via Stripe-hosted iframe (Stripe Elements). We never touch raw PANs. SAQ-D would apply only if we wrote our own card-capture forms."

hm emit evidence.recorded \
  --actor "human:sarah-l" \
  --evidence-id "ev-004" \
  --content "Incident report 2026-05-12: 3 long-lived API tokens leaked via a compromised developer laptop. Tokens had no expiry. No production payment data accessed (confirmed via audit logs). Root cause: tokens in .env files synced to personal cloud backup."

hm emit evidence.recorded \
  --actor "human:dan-r" \
  --evidence-id "ev-005" \
  --content "EU market research Q3 2026: 38% of surveyed e-commerce platforms are actively expanding to EU within 12 months. Top blocker cited: lack of localised payment methods (iDEAL, SOFORT, Bancontact)."

hm emit evidence.recorded \
  --actor "human:priya-k" \
  --evidence-id "ev-006" \
  --content "WorkOS incident post-mortem 2026-07-03: 47-minute outage affecting all WorkOS-hosted SSO. Root cause: DNS propagation failure during datacenter maintenance. Acme Payments saw 12% login failure rate during window."

hm emit evidence.recorded \
  --actor "human:sarah-l" \
  --evidence-id "ev-007" \
  --content "GDPR counsel memo 2026-09-02: Processing EU residents' payment data requires Article 30 record-keeping, DPO appointment if processing is large-scale, and a Transfer Impact Assessment for US→EU data flows. SAQ-A scope does not cover GDPR obligations."

hm emit evidence.recorded \
  --actor "agent:claude:arch" \
  --evidence-id "ev-008" \
  --content "AWS vs GCP TCO model (2026-01-15): 3-year TCO at projected scale — AWS \$1.2M, GCP \$1.05M, Azure \$1.35M. AWS wins on existing team familiarity (8/12 engineers have AWS certs) and breadth of managed services."

hm emit evidence.recorded \
  --actor "agent:claude:sec" \
  --evidence-id "ev-009" \
  --content "Auth scalability test 2026-06-18: stateless JWT with in-memory session cache fails at 520 concurrent users — cache eviction races cause 8% 401 error rate. Adding Redis eliminates errors but doubles p99 latency to 420 ms. WorkOS hosted auth baseline: 280 ms p99 at 1000 concurrent."

# ── Hypotheses ─────────────────────────────────────────────────────────────
echo "--- Hypotheses ---"

hm emit hypothesis.recorded \
  --actor "human:alex-m" \
  --hypothesis-id "hy-001" \
  --statement "Stateless JWT sessions will scale to 1000 concurrent users without requiring a shared cache (Redis or equivalent)."

hm emit hypothesis.recorded \
  --actor "agent:claude:sec" \
  --hypothesis-id "hy-002" \
  --statement "The 2026-05 API token leak was isolated to the compromised developer laptop and did not propagate to additional machines or cloud credentials."

hm emit hypothesis.recorded \
  --actor "human:dan-r" \
  --hypothesis-id "hy-003" \
  --statement "EU expansion can proceed under the existing PCI SAQ-A compliance posture without a separate GDPR audit, since payment data handling is unchanged."

# ── Decisions ──────────────────────────────────────────────────────────────
echo "--- Decisions ---"

# Chapter 1: Foundation (Jan 2026)

hm emit decision.proposed \
  --actor "human:alex-m" \
  --decision-id "d-001" \
  --title "Use Postgres as the primary transactional database" \
  --rationale "We need ACID transactions, row-level locking for concurrent payment writes, and mature JSON support for event payloads. MySQL had lock contention at 30+ connections in our bench. DynamoDB's aggregate query latency (>90 ms p99) is unacceptable for dashboard queries. Postgres on RDS managed instances gives us maintenance, failover, and backups without ops overhead." \
  --topic-keys "infrastructure" \
  --options "Postgres,MySQL,DynamoDB" \
  --chose "Postgres" \
  --evidence "ev-001"

hm emit decision.accepted \
  --actor "human:priya-k" \
  --decision "d-001"

hm emit decision.accepted \
  --actor "human:alex-m" \
  --decision "d-001"

hm emit decision.proposed \
  --actor "human:alex-m" \
  --decision-id "d-002" \
  --title "Run all infrastructure on AWS (primary cloud provider)" \
  --rationale "8 of 12 engineers hold AWS certifications. TCO model over 3 years shows AWS at \$1.2M vs GCP \$1.05M — the \$150K saving does not offset ~\$400K in retraining and tooling migration costs. AWS managed services (RDS, SQS, Lambda) cover our full stack without third-party glue. Azure ruled out: highest TCO and weakest managed payments ecosystem." \
  --topic-keys "infrastructure" \
  --options "AWS,GCP,Azure" \
  --chose "AWS" \
  --evidence "ev-008"

hm emit decision.accepted --actor "human:tom-b"   --decision "d-002"
hm emit decision.accepted --actor "human:priya-k" --decision "d-002"

hm emit decision.proposed \
  --actor "human:alex-m" \
  --decision-id "d-003" \
  --title "Use stateless JWT + in-memory session cache for API authentication" \
  --rationale "Auth0 and WorkOS both introduce vendor lock-in at a \$24K/year price point we cannot justify pre-Series-A. Stateless JWT avoids Redis dependency and keeps p99 auth latency under 50 ms in our test env. We will revisit at 500+ concurrent users." \
  --topic-keys "security" \
  --options "Stateless JWT + in-memory cache,WorkOS AuthKit,Auth0" \
  --chose "Stateless JWT + in-memory cache" \
  --hypotheses "hy-001"

hm emit decision.accepted --actor "human:priya-k" --decision "d-003"

hm emit decision.proposed \
  --actor "human:alex-m" \
  --decision-id "d-004" \
  --title "Use Rust as the primary backend language" \
  --rationale "Payments infra requires predictable latency, memory safety, and auditability. Go is a reasonable alternative but our two senior engineers have Rust production experience. Node is ruled out: async GC pauses are unacceptable for payment critical paths. We accept the steeper onboarding curve given the safety payoff." \
  --topic-keys "engineering" \
  --options "Rust,Go,Node.js" \
  --chose "Rust"

hm emit decision.accepted --actor "human:priya-k" --decision "d-004"
hm emit decision.accepted --actor "human:tom-b"   --decision "d-004"

# Chapter 2: Payment Provider (Mar 2026)

hm emit decision.proposed \
  --actor "human:dan-r" \
  --decision-id "d-005" \
  --title "Use Stripe as our primary payment processor" \
  --rationale "Stripe Connect covers our B2B marketplace model with direct charges and instant payouts. API latency p99 is 320 ms vs Adyen's 410 ms. Square requires a retail plan and does not support B2B-only. Stripe's ecosystem (Radar fraud, Revenue Recognition, Data Pipeline) means we buy vs build on ancillary products." \
  --topic-keys "payments" \
  --options "Stripe Connect,Adyen for Platforms,Square" \
  --chose "Stripe Connect" \
  --evidence "ev-002"

hm emit decision.accepted --actor "human:tom-b"    --decision "d-005"
hm emit decision.accepted --actor "human:alex-m"   --decision "d-005"
hm emit decision.accepted --actor "human:priya-k"  --decision "d-005"

hm emit decision.proposed \
  --actor "human:sarah-l" \
  --decision-id "d-006" \
  --title "Maintain PCI DSS SAQ-A compliance scope (not SAQ-D)" \
  --rationale "We use Stripe Elements exclusively for card capture — card data never touches our servers. Legal confirms SAQ-A applies. SAQ-D would require quarterly scans and annual on-site assessment, adding ~\$80K/year. Constraint: we must never render our own card-input forms." \
  --topic-keys "security,payments" \
  --options "SAQ-A (iframe only),SAQ-D (full cardholder data environment)" \
  --chose "SAQ-A (iframe only)" \
  --evidence "ev-003"

hm emit decision.accepted --actor "human:tom-b"  --decision "d-006"
hm emit decision.accepted --actor "human:alex-m" --decision "d-006"

hm emit decision.proposed \
  --actor "human:tom-b" \
  --decision-id "d-007" \
  --title "Charge platforms a flat 0.5% processing fee" \
  --rationale "Flat rate is simpler to explain to customers and reduces billing disputes. Tiered pricing optimises margin at high volume but we have no high-volume customers yet. Per-transaction favours low-value, high-frequency transactions which is not our current mix. We will revisit after first \$10M GMV." \
  --topic-keys "payments,product" \
  --options "Flat 0.5%,Tiered (0.3-0.8%),Per-transaction (\$0.10 + 0.2%)" \
  --chose "Flat 0.5%"

hm emit decision.accepted --actor "human:dan-r" --decision "d-007"

# Chapter 3: Security Incident — CONTESTED (May 2026)

hm emit decision.proposed \
  --actor "human:sarah-l" \
  --decision-id "d-008" \
  --title "Rotate all API tokens and enforce 2-FA on all developer accounts" \
  --rationale "Three long-lived tokens were leaked via a compromised laptop. Rotating every token is the only way to guarantee the blast radius is contained. 2-FA prevents credential reuse even if another laptop is compromised. Disruption to active integrations is acceptable given the security risk." \
  --topic-keys "security" \
  --hypotheses "hy-002" \
  --evidence "ev-004"

hm emit decision.accepted --actor "human:sarah-l" --decision "d-008"
hm emit decision.accepted --actor "human:dan-r"   --decision "d-008"
hm emit decision.rejected \
  --actor "human:alex-m" \
  --decision "d-008" \
  --reason "Token rotation will break active integrations for 12+ enterprise customers mid-billing-cycle. Prefer to require rotation only for the 3 affected tokens and roll out 2-FA with a 2-week notice period."

hm emit decision.proposed \
  --actor "human:sarah-l" \
  --decision-id "d-009" \
  --title "Disable API-key authentication; require JWT for all API access" \
  --rationale "API keys are static secrets with no expiry. Post-incident analysis shows long-lived keys are the primary risk surface. JWT tokens expire in 24h and are tied to a session. Deprecating API keys closes that attack vector permanently." \
  --topic-keys "security"

hm emit decision.accepted --actor "human:sarah-l"    --decision "d-009"
hm emit decision.accepted --actor "agent:claude:sec"  --decision "d-009"
hm emit decision.rejected \
  --actor "human:priya-k" \
  --decision "d-009" \
  --reason "API keys are required by 7 integrations that cannot rotate to JWT in the proposed 2-week window. A deprecation timeline of 6 months with migration tooling is required."

# Chapter 4: Auth Supersession Chain (Jun–Jul 2026)
# D-003 → D-010 → D-011

hm emit decision.proposed \
  --actor "human:alex-m" \
  --decision-id "d-010" \
  --title "Migrate API authentication to WorkOS AuthKit" \
  --rationale "Load testing at 520 concurrent users revealed JWT + in-memory cache causes 8% 401 errors due to cache eviction races (hypothesis hy-001 refuted). Adding Redis eliminates errors but doubles auth latency. WorkOS AuthKit at 280 ms p99 at 1000 concurrent is superior. Post-Series-A budget now justifies the \$24K/year cost. Supersedes d-003." \
  --topic-keys "security,infrastructure" \
  --options "WorkOS AuthKit,Redis-backed JWT,Auth0" \
  --chose "WorkOS AuthKit" \
  --hypotheses "hy-001" \
  --evidence "ev-009"

hm emit decision.accepted --actor "human:priya-k" --decision "d-010"
hm emit decision.accepted --actor "human:sarah-l" --decision "d-010"
hm emit decision.superseded --old "d-003" --new "d-010"

hm emit decision.proposed \
  --actor "human:priya-k" \
  --decision-id "d-011" \
  --title "Add SSO fallback provider (Okta) alongside WorkOS for auth resilience" \
  --rationale "WorkOS experienced a 47-minute outage on 2026-07-03 that caused 12% login failures. Single auth provider is a critical availability risk. Adding Okta as a hot standby allows failover in under 5 minutes. Supersedes d-010." \
  --topic-keys "security,infrastructure" \
  --options "WorkOS + Okta hot standby,WorkOS only (accept risk),Build in-house SSO abstraction layer" \
  --chose "WorkOS + Okta hot standby" \
  --evidence "ev-006"

hm emit decision.accepted --actor "human:alex-m" --decision "d-011"
hm emit decision.accepted --actor "human:tom-b"  --decision "d-011"
hm emit decision.superseded --old "d-010" --new "d-011"

# Chapter 4b: DB Supersession Chain (Jun–Sep 2026)
# D-001 → D-012 → D-013

hm emit decision.proposed \
  --actor "human:priya-k" \
  --decision-id "d-012" \
  --title "Add a Postgres read replica for dashboard and analytics queries" \
  --rationale "Dashboard queries now account for 35% of DB load and are causing p95 latency degradation on write paths. A read replica isolates read load. This is the first step on the DB scaling ladder; connection pooling and sharding are deferred. Extends d-001." \
  --topic-keys "infrastructure" \
  --options "Postgres primary + read replica,Read-through cache (Redis)"  \
  --chose "Postgres primary + read replica"

hm emit decision.accepted --actor "human:alex-m" --decision "d-012"
hm emit decision.superseded --old "d-001" --new "d-012"

hm emit decision.proposed \
  --actor "human:priya-k" \
  --decision-id "d-013" \
  --title "Add PgBouncer connection pooler in front of Postgres (primary + replica)" \
  --rationale "After read replica deploy, we still see connection exhaustion during peak (>180 connections). PgBouncer in transaction-pooling mode lets us serve 1000+ app connections over 50 backend connections. Eliminates connection exhaustion without schema changes. Extends d-012." \
  --topic-keys "infrastructure" \
  --options "PgBouncer transaction mode,RDS Proxy (AWS managed),Shard the database" \
  --chose "PgBouncer transaction mode"

hm emit decision.accepted --actor "human:alex-m"       --decision "d-013"
hm emit decision.accepted --actor "agent:claude:arch"   --decision "d-013"
hm emit decision.superseded --old "d-012" --new "d-013"

# Chapter 5: Product Pivot (Aug–Oct 2026)

# Decision request first, then the decision

hm emit decision.requested \
  --actor "human:dan-r" \
  --topic-keys "product,payments" \
  --reason "Several enterprise prospects have asked whether we plan to accept Bitcoin or Ethereum payments. No decision made. Do we add crypto payment support in 2026, and if so what scope?" \
  --priority P2 \
  --decision-id "dr-001"

hm emit decision.proposed \
  --actor "human:tom-b" \
  --decision-id "d-014" \
  --title "Do not add cryptocurrency payment support in 2026" \
  --rationale "Crypto volume in B2B e-commerce is < 0.3% of GMV per Stripe data. Compliance burden (FinCEN MSB registration, BitLicense NY) is disproportionate. Engineering cost is 3+ months. Deferred to 2027 reassessment when regulatory clarity improves." \
  --topic-keys "product,payments" \
  --options "Add crypto support (BTC + ETH),Defer to 2027" \
  --chose "Defer to 2027"

hm emit decision.accepted --actor "human:dan-r"   --decision "d-014"
hm emit decision.accepted --actor "human:sarah-l" --decision "d-014"

hm emit decision.proposed \
  --actor "human:tom-b" \
  --decision-id "d-015" \
  --title "Expand product to EU market (starting with Germany + Netherlands)" \
  --rationale "38% of surveyed platforms are expanding to EU within 12 months. iDEAL (NL) and SOFORT (DE) are top-requested payment methods. Stripe supports both natively. We target Q1 2027 GA; compliance groundwork (GDPR, local payment methods) starts Q4 2026." \
  --topic-keys "product,payments" \
  --options "EU expansion (DE + NL first),APAC expansion,Deepen US coverage only" \
  --chose "EU expansion (DE + NL first)" \
  --evidence "ev-005"

hm emit decision.accepted --actor "human:dan-r"   --decision "d-015"
hm emit decision.accepted --actor "human:priya-k" --decision "d-015"

hm emit decision.proposed \
  --actor "human:priya-k" \
  --decision-id "d-016" \
  --title "Deprecate Webhook API v1; require migration to Webhook API v2" \
  --rationale "Webhook v1 lacks idempotency keys, has no retry logic, and exposes raw event payloads that caused two data-consistency incidents in Q2. v2 fixes all three. We will give customers 6 months (until 2027-04-01) to migrate. v1 will be disabled, not sunset slowly, to avoid split maintenance burden." \
  --topic-keys "product,engineering"

hm emit decision.accepted --actor "human:dan-r"  --decision "d-016"
hm emit decision.accepted --actor "human:alex-m" --decision "d-016"

hm emit decision.proposed \
  --actor "human:sarah-l" \
  --decision-id "d-017" \
  --title "Host EU customer data exclusively in AWS eu-central-1 (Frankfurt)" \
  --rationale "GDPR data residency requires EU personal data stay in the EU or an adequacy-listed country. eu-central-1 (Frankfurt) and eu-west-1 (Dublin) both qualify. Frankfurt has lower latency to our primary DE + NL markets (13 ms vs 22 ms from Dublin). All EU tenants get an isolated RDS instance in eu-central-1; no cross-region replication of personal data." \
  --topic-keys "infrastructure,security,payments" \
  --options "eu-central-1 (Frankfurt),eu-west-1 (Dublin)" \
  --chose "eu-central-1 (Frankfurt)" \
  --evidence "ev-007"

hm emit decision.accepted --actor "human:tom-b"  --decision "d-017"
hm emit decision.accepted --actor "human:alex-m" --decision "d-017"

# Chapter 6: Compliance & Scale (Oct–Dec 2026)

hm emit decision.proposed \
  --actor "human:tom-b" \
  --decision-id "d-018" \
  --title "Appoint external DPO firm for GDPR compliance" \
  --rationale "GDPR Article 37 mandates a Data Protection Officer when processing large-scale sensitive data. In-house DPO would cost ~\$180K/year (loaded) plus 3-6 months to hire. External DPO firm at \$36K/year provides immediate coverage, regulatory network, and scales with headcount. Selected firm: DataGuard (EU-specialist)." \
  --topic-keys "security,product" \
  --options "External DPO firm (DataGuard),In-house DPO hire" \
  --chose "External DPO firm (DataGuard)" \
  --evidence "ev-007"

hm emit decision.accepted --actor "human:sarah-l" --decision "d-018"
hm emit decision.accepted --actor "human:dan-r"   --decision "d-018"

hm emit decision.proposed \
  --actor "human:sarah-l" \
  --decision-id "d-019" \
  --title "Upgrade to PCI DSS Level 1 compliance ahead of EU launch" \
  --rationale "EU enterprise customers require PCI Level 1 (Report on Compliance, not self-assessment). Current SAQ-A is insufficient for contracts above €1M GMV/year. Level 1 audit cost (\$120K) is offset by unlocking the enterprise tier. Audit scope: Stripe integration, tokenisation flow, audit logging." \
  --topic-keys "security,payments" \
  --evidence "ev-003"

hm emit decision.accepted --actor "human:tom-b"  --decision "d-019"
hm emit decision.accepted --actor "human:alex-m" --decision "d-019"

hm emit decision.proposed \
  --actor "human:alex-m" \
  --decision-id "d-020" \
  --title "Reorganise engineering into a Platform team and two Product teams" \
  --rationale "Single-team structure creates coordination overhead as we approach 20 engineers. Platform team owns: payments infra, auth, DB, deployment. Product teams (2) own: Merchant Dashboard and Partner API. Avoids a third-tier hierarchy that would slow decisions." \
  --topic-keys "engineering" \
  --options "Platform + 2 product teams,Feature squads (no platform),Functional departments (Eng / Product / Infra)" \
  --chose "Platform + 2 product teams"

hm emit decision.accepted --actor "human:tom-b"   --decision "d-020"
hm emit decision.accepted --actor "human:priya-k" --decision "d-020"

hm emit decision.proposed \
  --actor "human:dan-r" \
  --decision-id "d-021" \
  --title "Move merchant onboarding to self-serve (remove white-glove prerequisite)" \
  --rationale "White-glove onboarding caps throughput at 8 new merchants/month (3 sales engineers, full-day sessions). Self-serve with docs + sandbox targets 50+ merchants/month. B2B SaaS comps show self-serve achieves comparable activation rates when sandbox is frictionless. Requires: improved CLI quickstart, hosted sandbox env, video walkthroughs." \
  --topic-keys "product,engineering" \
  --options "Self-serve + async support,White-glove only (status quo),Hybrid (self-serve + optional white-glove)" \
  --chose "Self-serve + async support"

hm emit decision.accepted --actor "human:tom-b"  --decision "d-021"
hm emit decision.accepted --actor "human:alex-m" --decision "d-021"

hm emit decision.proposed \
  --actor "human:alex-m" \
  --decision-id "d-022" \
  --title "Adopt Temporal.io for all long-running payment workflow orchestration" \
  --rationale "Payout workflows, dispute handling, and reconciliation jobs all need durable execution with retries. Current approach (SQS + Lambda + ad-hoc state in Postgres) has caused 4 data-consistency incidents in 2026. Temporal provides durable saga patterns, built-in replay, and a workflow history UI. Replaces ad-hoc queue/cron approach." \
  --topic-keys "infrastructure,payments,engineering" \
  --options "Temporal.io (self-hosted),AWS Step Functions,Maintain current SQS+Lambda approach" \
  --chose "Temporal.io (self-hosted)"

hm emit decision.accepted --actor "human:priya-k"     --decision "d-022"
hm emit decision.accepted --actor "agent:claude:arch"  --decision "d-022"

hm emit decision.proposed \
  --actor "human:sarah-l" \
  --decision-id "d-023" \
  --title "Freeze EU beta data in us-east-1; defer eu-central-1 migration to GA" \
  --rationale "Early EU beta (3 pilot customers) showed data volumes too small to justify a dedicated eu-central-1 instance. Legal confirmed pilot data is covered by SCCs under our existing DPA. We will revisit at 100+ EU tenants. Supersedes d-017 for beta phase only; d-017 remains the target for GA." \
  --topic-keys "infrastructure,security"

hm emit decision.accepted --actor "human:tom-b" --decision "d-023"
hm emit decision.rejected \
  --actor "human:dan-r" \
  --decision "d-023" \
  --reason "Freezing EU data in us-east-1, even temporarily, creates a compliance narrative risk if leaked. Prefer a minimal eu-central-1 RDS instance even for beta."

# (Note: d-023 is contested; we do NOT call supersede because supersede is
# tracked on d-017 and only if d-023 truly supersedes it, which is beta-scoped.)

hm emit decision.proposed \
  --actor "human:priya-k" \
  --decision-id "d-024" \
  --title "Use Sentry for error monitoring and Datadog for infrastructure metrics" \
  --rationale "Two tools rather than one: Sentry owns application errors (stack traces, release tracking, user context); Datadog owns infra metrics, APM, and logs. Evaluated New Relic (all-in-one) — worse DX for Rust, higher cost at our scale. Combined Sentry+Datadog is \$18K/year vs New Relic \$22K." \
  --topic-keys "engineering,infrastructure" \
  --options "Sentry + Datadog,New Relic (all-in-one),Grafana OSS stack (self-hosted)" \
  --chose "Sentry + Datadog"

hm emit decision.accepted --actor "human:alex-m" --decision "d-024"

hm emit decision.proposed \
  --actor "human:priya-k" \
  --decision-id "d-025" \
  --title "Adopt LaunchDarkly for all new feature rollouts" \
  --rationale "EU expansion and PCI Level 1 rollout both need progressive deployment with instant kill-switches. Current approach (hard-coded env vars) cannot target by tenant or percentage. LaunchDarkly at \$12K/year with SDK support for Rust. Custom implementation estimated at 6 weeks." \
  --topic-keys "engineering,product" \
  --options "LaunchDarkly,Build in-house flag service,Unleash OSS (self-hosted)" \
  --chose "LaunchDarkly"

hm emit decision.accepted --actor "human:dan-r"  --decision "d-025"
hm emit decision.accepted --actor "human:alex-m" --decision "d-025"

# ── Blockers ───────────────────────────────────────────────────────────────
echo "--- Blockers ---"

hm emit blocker.reported \
  --actor "human:sarah-l" \
  --blocker-id "bl-001" \
  --description "PCI Level 1 audit required before EU enterprise onboarding. Cannot sign contracts above €1M GMV/year until Report on Compliance is issued." \
  --priority P1 \
  --decision "d-015"

hm emit blocker.resolved \
  --actor "human:sarah-l" \
  --blocker "bl-001"

hm emit blocker.reported \
  --actor "human:sarah-l" \
  --blocker-id "bl-002" \
  --description "DPO appointment pending. Cannot publish GDPR privacy notice or sign EU data processing agreements until DPO is named." \
  --priority P1 \
  --decision "d-015"

hm emit blocker.resolved \
  --actor "human:sarah-l" \
  --blocker "bl-002"

# ── Semantic relations ─────────────────────────────────────────────────────
echo "--- Relations ---"

# SAQ-A posture SUPPORTS Stripe choice (they're co-dependent)
hm emit relation.added \
  --actor "human:sarah-l" \
  --kind SUPPORTS \
  --from "d-006" \
  --to "d-005"

# EU expansion ASSUMES eu-central-1 data residency
hm emit relation.added \
  --actor "human:alex-m" \
  --kind ASSUMES \
  --from "d-015" \
  --to "d-017"

# PCI Level 1 SUPPORTS EU expansion (unblocks enterprise EU contracts)
hm emit relation.added \
  --actor "human:sarah-l" \
  --kind SUPPORTS \
  --from "d-019" \
  --to "d-015"

# DPO appointment SUPPORTS EU expansion (unblocks GDPR compliance)
hm emit relation.added \
  --actor "human:sarah-l" \
  --kind SUPPORTS \
  --from "d-018" \
  --to "d-015"

# Temporal SUPPORTS Stripe (better orchestration for Stripe payment workflows)
hm emit relation.added \
  --actor "human:alex-m" \
  --kind SUPPORTS \
  --from "d-022" \
  --to "d-005"

# Self-serve onboarding ASSUMES webhook v2 (simpler integration surface)
hm emit relation.added \
  --actor "human:dan-r" \
  --kind ASSUMES \
  --from "d-021" \
  --to "d-016"

# LaunchDarkly flags SUPPORTS EU expansion rollout
hm emit relation.added \
  --actor "human:priya-k" \
  --kind SUPPORTS \
  --from "d-025" \
  --to "d-015"

# Disable API keys REFUTES original JWT-only assumption in d-003
hm emit relation.added \
  --actor "human:sarah-l" \
  --kind REFUTES \
  --from "d-009" \
  --to "d-003"

# Hypothesis hy-003 (EU without GDPR audit) — attach evidence refuting it
hm emit relation.attach_evidence \
  --from "hy-003" \
  --evidence "ev-007"

# ── Hypothesis resolutions ─────────────────────────────────────────────────
# hy-001 is refuted (JWT didn't scale) — captured via hypotheses link on d-010
# hy-002 is confirmed (leak was isolated) — captured via hypotheses link on d-008
# hy-003 is refuted (EU needs GDPR audit) — refuted via ev-007 attached above

# ── Scorer events (gated on EMIT_SCORES=true) ─────────────────────────────
if [ "$EMIT_SCORES" = "true" ]; then
  echo "--- Scorer events (decision.scored) ---"
  echo "NOTE: scorer events require hivemind-m2-shared-backend-lives-uuq9.19 merged"

  # High-importance decisions
  hm emit decision.scored \
    --actor "agent:hivemind:scorer" \
    --decision "d-005" \
    --scorer-model "claude-opus-4-8" \
    --weight-version "v1" \
    --stakes 110.0 --stakes-explanation "Primary payment processor: choosing wrong vendor affects all revenue" \
    --irreversibility 0.70 --irreversibility-explanation "Migration possible but costly (3+ months, customer disruption)" \
    --actionability 1.0 --actionability-explanation "Decision is fully in our control, can be made today" \
    --framing 0.85 --alternatives 0.80 --information 0.82 --reasoning 0.79 \
    --values-tradeoffs 0.75 --bias-exposure 0.72 --calibration 0.88

  hm emit decision.scored \
    --actor "agent:hivemind:scorer" \
    --decision "d-002" \
    --scorer-model "claude-opus-4-8" \
    --weight-version "v1" \
    --stakes 90.0 --stakes-explanation "Cloud platform locks in tooling, pricing, compliance posture for years" \
    --irreversibility 0.75 --irreversibility-explanation "Migration possible but 6-12 month effort" \
    --actionability 1.0 --actionability-explanation "Standard infrastructure decision" \
    --framing 0.88 --alternatives 0.82 --information 0.90 --reasoning 0.84 \
    --values-tradeoffs 0.80 --bias-exposure 0.78 --calibration 0.85

  hm emit decision.scored \
    --actor "agent:hivemind:scorer" \
    --decision "d-015" \
    --scorer-model "claude-opus-4-8" \
    --weight-version "v1" \
    --stakes 130.0 --stakes-explanation "Geographic expansion affects revenue ceiling, compliance burden, team structure" \
    --irreversibility 0.60 --irreversibility-explanation "Can pause expansion if needed, but regulatory obligations persist" \
    --actionability 0.90 --actionability-explanation "Requires compliance groundwork not yet complete" \
    --framing 0.78 --alternatives 0.72 --information 0.80 --reasoning 0.76 \
    --values-tradeoffs 0.70 --bias-exposure 0.65 --calibration 0.74

  hm emit decision.scored \
    --actor "agent:hivemind:scorer" \
    --decision "d-008" \
    --scorer-model "claude-opus-4-8" \
    --weight-version "v1" \
    --stakes 95.0 --stakes-explanation "Security incident response: wrong call could expose customer payment data" \
    --irreversibility 0.50 --irreversibility-explanation "Token rotation is reversible; 2-FA enforcement less so" \
    --actionability 1.0 --actionability-explanation "Fully within our control, time-sensitive" \
    --framing 0.55 --alternatives 0.48 --information 0.62 --reasoning 0.52 \
    --values-tradeoffs 0.42 --bias-exposure 0.35 --calibration 0.55

  hm emit decision.scored \
    --actor "agent:hivemind:scorer" \
    --decision "d-003" \
    --scorer-model "claude-opus-4-8" \
    --weight-version "v1" \
    --stakes 60.0 --stakes-explanation "Auth architecture underpins all API security" \
    --irreversibility 0.50 --irreversibility-explanation "Replaced 5 months later proving reversible" \
    --actionability 1.0 --actionability-explanation "Standard auth architecture choice" \
    --framing 0.40 --alternatives 0.32 --information 0.35 --reasoning 0.38 \
    --values-tradeoffs 0.42 --bias-exposure 0.28 --calibration 0.30

  hm emit decision.scored \
    --actor "agent:hivemind:scorer" \
    --decision "d-001" \
    --scorer-model "claude-opus-4-8" \
    --weight-version "v1" \
    --stakes 85.0 --stakes-explanation "Database choice constrains query patterns, scaling, and ops for years" \
    --irreversibility 0.80 --irreversibility-explanation "Database migrations at scale are expensive and risky" \
    --actionability 1.0 --actionability-explanation "Greenfield decision" \
    --framing 0.84 --alternatives 0.80 --information 0.88 --reasoning 0.82 \
    --values-tradeoffs 0.76 --bias-exposure 0.75 --calibration 0.85

  hm emit decision.scored \
    --actor "agent:hivemind:scorer" \
    --decision "d-024" \
    --scorer-model "claude-opus-4-8" \
    --weight-version "v1" \
    --stakes 18.0 --stakes-explanation "Tooling choice; low stakes, easily changed" \
    --irreversibility 0.30 --irreversibility-explanation "SDK integrations take a sprint to swap" \
    --actionability 1.0 --actionability-explanation "Straightforward vendor selection" \
    --framing 0.78 --alternatives 0.74 --information 0.72 --reasoning 0.76 \
    --values-tradeoffs 0.70 --bias-exposure 0.68 --calibration 0.78

  echo "Scorer events emitted."
fi

# ── Summary ────────────────────────────────────────────────────────────────
echo ""
echo "=== Load complete ==="
echo "Verify with:"
echo "  $HM $HM_FLAGS query decisions --json | jq 'length'"
echo "  $HM $HM_FLAGS query graph --format dot | head -30"
