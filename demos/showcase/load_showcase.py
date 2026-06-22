#!/usr/bin/env python3
"""
load_showcase.py — Populate a HiveMind ledger with the Acme Payments
showcase corpus for SPA + spectral-map demonstration.

Usage:
  # From the repo root:
  python3 demos/showcase/load_showcase.py

  # Override defaults:
  HIVEMIND_DIR=./demo-ledger/ TENANT=demo python3 demos/showcase/load_showcase.py

  # Also emit scorer events (requires hivemind-m2-shared-backend-lives-uuq9.19 merged):
  EMIT_SCORES=true python3 demos/showcase/load_showcase.py

Prerequisites:
  cargo build --release --bin hivemind   (or 'hivemind' on PATH)
  pip install pyyaml  (if loading from YAML; not required for the inline corpus)
"""

import os
import subprocess
import sys
from pathlib import Path

# ── Config ──────────────────────────────────────────────────────────────────
HIVEMIND_DIR = os.environ.get("HIVEMIND_DIR", "./hivemind/")
TENANT = os.environ.get("TENANT", "demo")
EMIT_SCORES = os.environ.get("EMIT_SCORES", "false").lower() == "true"

# Locate binary
def find_hivemind():
    for candidate in ["hivemind", "./target/release/hivemind", "./target/debug/hivemind"]:
        try:
            subprocess.run(
                [candidate, "--help"],
                capture_output=True,
                check=True,
            )
            return candidate
        except (FileNotFoundError, subprocess.CalledProcessError):
            pass
    print("ERROR: hivemind binary not found. Run: cargo build --bin hivemind", file=sys.stderr)
    sys.exit(1)

HM = find_hivemind()
BASE_FLAGS = ["--hivemind-dir", HIVEMIND_DIR, "--tenant", TENANT]

def hm(*args, actor=None):
    """Run a hivemind command, return stripped stdout."""
    cmd = [HM]
    if actor:
        cmd += ["--actor", actor]
    cmd += list(BASE_FLAGS)
    cmd += list(args)
    result = subprocess.run(cmd, capture_output=True, text=True, check=True)
    return result.stdout.strip()

def emit_evidence(actor_id, content, source=None, source_ref=None):
    args = ["emit", "evidence.recorded", "--actor-id", actor_id, "--content", content]
    if source:
        args += ["--source", source]
    if source_ref:
        args += ["--source-ref", source_ref]
    return hm(*args)

def emit_hypothesis(actor_id, statement, source=None, source_ref=None):
    args = ["emit", "hypothesis.recorded", "--actor-id", actor_id, "--statement", statement]
    if source:
        args += ["--source", source]
    if source_ref:
        args += ["--source-ref", source_ref]
    return hm(*args)

def emit_decision(actor, title, rationale, topic_keys, options=None, chose=None,
                  evidence=None, hypotheses=None):
    args = ["emit", "decision.proposed", "--title", title, "--rationale", rationale,
            "--topic-keys", ",".join(topic_keys)]
    if options:
        args += ["--options", ",".join(options)]
    if chose:
        args += ["--chose", chose]
    if evidence:
        args += ["--evidence", ",".join(evidence)]
    if hypotheses:
        args += ["--hypotheses", ",".join(hypotheses)]
    return hm(*args, actor=actor)

def accept(actor, decision_id):
    hm("emit", "decision.accepted", "--decision-id", decision_id, actor=actor)

def reject(actor, decision_id):
    hm("emit", "decision.rejected", "--decision-id", decision_id, actor=actor)

def disagree(actor, decision_id, reason):
    hm("disagree", "--decision", decision_id, "--reason", reason, actor=actor)

def supersede(old_id, new_id):
    hm("emit", "decision.superseded", "--old", old_id, "--new", new_id)

def add_relation(kind, from_id, to_id):
    # CLI supports: supports, refutes, based-on
    hm("emit", "relation.added", "--kind", kind, "--from", from_id, "--to", to_id)

def attach_evidence(decision_id, evidence_id):
    hm("emit", "relation.attach_evidence", "--decision-id", decision_id,
       "--evidence-id", evidence_id)

# ── ID registry ─────────────────────────────────────────────────────────────
# Maps corpus logical names → system-generated IDs
ids = {}

def register(name, system_id):
    ids[name] = system_id
    print(f"  {name} → {system_id}")
    return system_id

# ── Main ────────────────────────────────────────────────────────────────────
def main():
    print(f"=== Acme Payments showcase corpus loader ===")
    print(f"  dir:    {HIVEMIND_DIR}")
    print(f"  tenant: {TENANT}")
    print(f"  scores: {EMIT_SCORES}")
    print()

    # ── Evidence ────────────────────────────────────────────────────────────
    print("--- Evidence ---")

    register("ev-001", emit_evidence(
        "agent:claude:arch", source="agent",
        content=(
            "Load-test run 2026-01-08: Postgres sustained 220 TPS/tenant at 50 concurrent "
            "connections on db.t3.xlarge. MySQL reached 190 TPS but showed lock contention "
            "at 30+ connections. DynamoDB showed p99 latency > 90 ms on aggregate queries."
        ),
        source_ref="gdoc:arch-benchmarks-jan-2026",
    ))

    register("ev-002", emit_evidence(
        "agent:claude:arch", source="agent",
        content=(
            "Stripe API integration spike: p99 charge latency 320 ms, p50 180 ms across "
            "10k sample transactions. Adyen average 410 ms, higher variance. Square "
            "unavailable for B2B-only use-case without retail plan."
        ),
        source_ref="gdoc:payments-api-spike-mar-2026",
    ))

    register("ev-003", emit_evidence(
        "human:sarah-l", source="human",
        content=(
            "Legal memo 2026-03-15: SAQ-A applies when card data is handled exclusively "
            "via Stripe-hosted iframe (Stripe Elements). We never touch raw PANs. "
            "SAQ-D would apply only if we wrote our own card-capture forms."
        ),
        source_ref="gdoc:pci-legal-memo-mar-2026",
    ))

    register("ev-004", emit_evidence(
        "human:sarah-l", source="human",
        content=(
            "Incident report 2026-05-12: 3 long-lived API tokens leaked via a compromised "
            "developer laptop. Tokens had no expiry. No production payment data accessed "
            "(confirmed via audit logs). Root cause: tokens in .env files synced to personal "
            "cloud backup."
        ),
        source_ref="slack:C07INCIDENT:1747046400",
    ))

    register("ev-005", emit_evidence(
        "human:dan-r", source="human",
        content=(
            "EU market research Q3 2026: 38% of surveyed e-commerce platforms are actively "
            "expanding to EU within 12 months. Top blocker cited: lack of localised payment "
            "methods (iDEAL, SOFORT, Bancontact)."
        ),
        source_ref="gdoc:eu-market-research-aug-2026",
    ))

    register("ev-006", emit_evidence(
        "human:priya-k", source="human",
        content=(
            "WorkOS incident post-mortem 2026-07-03: 47-minute outage affecting all "
            "WorkOS-hosted SSO. Root cause: DNS propagation failure during datacenter "
            "maintenance. Acme Payments saw 12% login failure rate during window."
        ),
        source_ref="gdoc:workos-outage-postmortem-jul-2026",
    ))

    register("ev-007", emit_evidence(
        "human:sarah-l", source="human",
        content=(
            "GDPR counsel memo 2026-09-02: Processing EU residents' payment data requires "
            "Article 30 record-keeping, DPO appointment if processing is large-scale, and a "
            "Transfer Impact Assessment for US→EU data flows. SAQ-A scope does not cover "
            "GDPR obligations."
        ),
        source_ref="gdoc:gdpr-counsel-memo-sep-2026",
    ))

    register("ev-008", emit_evidence(
        "agent:claude:arch", source="agent",
        content=(
            "AWS vs GCP TCO model (2026-01-15): 3-year TCO at projected scale — AWS $1.2M, "
            "GCP $1.05M, Azure $1.35M. AWS wins on existing team familiarity (8/12 engineers "
            "have AWS certs) and breadth of managed services (RDS, SQS, Lambda all needed)."
        ),
        source_ref="gdoc:cloud-tco-jan-2026",
    ))

    register("ev-009", emit_evidence(
        "agent:claude:sec", source="agent",
        content=(
            "Auth scalability test 2026-06-18: stateless JWT with in-memory session cache "
            "fails at 520 concurrent users — cache eviction races cause 8% 401 error rate. "
            "Adding Redis eliminates errors but doubles p99 latency to 420 ms. WorkOS hosted "
            "auth baseline: 280 ms p99 at 1000 concurrent."
        ),
        source_ref="gdoc:auth-scale-test-jun-2026",
    ))

    # ── Hypotheses ──────────────────────────────────────────────────────────
    print("\n--- Hypotheses ---")

    register("hy-001", emit_hypothesis(
        "human:alex-m", source="human",
        statement=(
            "Stateless JWT sessions will scale to 1000 concurrent users without requiring "
            "a shared cache (Redis or equivalent)."
        ),
    ))

    register("hy-002", emit_hypothesis(
        "agent:claude:sec", source="agent",
        statement=(
            "The 2026-05 API token leak was isolated to the compromised developer laptop "
            "and did not propagate to additional machines or cloud credentials."
        ),
    ))

    register("hy-003", emit_hypothesis(
        "human:dan-r", source="human",
        statement=(
            "EU expansion can proceed under the existing PCI SAQ-A compliance posture "
            "without a separate GDPR audit, since payment data handling is unchanged."
        ),
    ))

    # ── Decisions ───────────────────────────────────────────────────────────
    print("\n--- Chapter 1: Foundation (Jan 2026) ---")

    register("d-001", emit_decision(
        "human:alex-m",
        title="Use Postgres as the primary transactional database",
        rationale=(
            "We need ACID transactions, row-level locking for concurrent payment writes, and "
            "mature JSON support for event payloads. MySQL had lock contention at 30+ connections "
            "in our bench. DynamoDB's aggregate query latency (>90 ms p99) is unacceptable for "
            "dashboard queries. Postgres on RDS managed instances gives us maintenance, failover, "
            "and backups without ops overhead."
        ),
        topic_keys=["infrastructure"],
        options=["Postgres", "MySQL", "DynamoDB"],
        chose="Postgres",
        evidence=[ids["ev-001"]],
    ))
    accept("human:priya-k", ids["d-001"])
    accept("human:alex-m", ids["d-001"])

    register("d-002", emit_decision(
        "human:alex-m",
        title="Run all infrastructure on AWS (primary cloud provider)",
        rationale=(
            "8 of 12 engineers hold AWS certifications. TCO model over 3 years shows AWS at "
            "$1.2M vs GCP $1.05M — the $150K saving does not offset ~$400K in retraining and "
            "tooling migration costs. AWS managed services (RDS, SQS, Lambda) cover our full "
            "stack without third-party glue. Azure ruled out: highest TCO and weakest managed "
            "payments ecosystem."
        ),
        topic_keys=["infrastructure"],
        options=["AWS", "GCP", "Azure"],
        chose="AWS",
        evidence=[ids["ev-008"]],
    ))
    accept("human:tom-b", ids["d-002"])
    accept("human:priya-k", ids["d-002"])

    register("d-003", emit_decision(
        "human:alex-m",
        title="Use stateless JWT + in-memory session cache for API authentication",
        rationale=(
            "Auth0 and WorkOS both introduce vendor lock-in at a $24K/year price point we "
            "cannot justify pre-Series-A. Stateless JWT avoids Redis dependency and keeps p99 "
            "auth latency under 50 ms in our test env. We will revisit at 500+ concurrent users."
        ),
        topic_keys=["security"],
        options=["Stateless JWT + in-memory cache", "WorkOS AuthKit", "Auth0"],
        chose="Stateless JWT + in-memory cache",
        hypotheses=[ids["hy-001"]],
    ))
    accept("human:priya-k", ids["d-003"])

    register("d-004", emit_decision(
        "human:alex-m",
        title="Use Rust as the primary backend language",
        rationale=(
            "Payments infra requires predictable latency, memory safety, and auditability. "
            "Go is a reasonable alternative but our two senior engineers have Rust production "
            "experience. Node is ruled out: async GC pauses are unacceptable for payment "
            "critical paths. We accept the steeper onboarding curve given the safety payoff."
        ),
        topic_keys=["engineering"],
        options=["Rust", "Go", "Node.js"],
        chose="Rust",
    ))
    accept("human:priya-k", ids["d-004"])
    accept("human:tom-b", ids["d-004"])

    print("\n--- Chapter 2: Payment Provider (Mar 2026) ---")

    register("d-005", emit_decision(
        "human:dan-r",
        title="Use Stripe as our primary payment processor",
        rationale=(
            "Stripe Connect covers our B2B marketplace model with direct charges and instant "
            "payouts. API latency p99 is 320 ms vs Adyen's 410 ms. Square requires a retail "
            "plan and does not support B2B-only. Stripe's ecosystem (Radar fraud, Revenue "
            "Recognition, Data Pipeline) means we buy vs build on ancillary products."
        ),
        topic_keys=["payments"],
        options=["Stripe Connect", "Adyen for Platforms", "Square"],
        chose="Stripe Connect",
        evidence=[ids["ev-002"]],
    ))
    accept("human:tom-b", ids["d-005"])
    accept("human:alex-m", ids["d-005"])
    accept("human:priya-k", ids["d-005"])

    register("d-006", emit_decision(
        "human:sarah-l",
        title="Maintain PCI DSS SAQ-A compliance scope (not SAQ-D)",
        rationale=(
            "We use Stripe Elements exclusively for card capture — card data never touches "
            "our servers. Legal confirms SAQ-A applies. SAQ-D would require quarterly scans "
            "and annual on-site assessment, adding ~$80K/year. Constraint: we must never "
            "render our own card-input forms."
        ),
        topic_keys=["security", "payments"],
        options=["SAQ-A (iframe only)", "SAQ-D (full cardholder data environment)"],
        chose="SAQ-A (iframe only)",
        evidence=[ids["ev-003"]],
    ))
    accept("human:tom-b", ids["d-006"])
    accept("human:alex-m", ids["d-006"])

    register("d-007", emit_decision(
        "human:tom-b",
        title="Charge platforms a flat 0.5% processing fee",
        rationale=(
            "Flat rate is simpler to explain to customers and reduces billing disputes. "
            "Tiered pricing optimises margin at high volume but we have no high-volume "
            "customers yet. Per-transaction favours low-value, high-frequency transactions "
            "which is not our current mix. We will revisit after first $10M GMV."
        ),
        topic_keys=["payments", "product"],
        options=["Flat 0.5%", "Tiered (0.3-0.8%)", "Per-transaction ($0.10 + 0.2%)"],
        chose="Flat 0.5%",
    ))
    accept("human:dan-r", ids["d-007"])

    print("\n--- Chapter 3: Security Incident — CONTESTED (May 2026) ---")

    register("d-008", emit_decision(
        "human:sarah-l",
        title="Rotate all API tokens and enforce 2-FA on all developer accounts",
        rationale=(
            "Three long-lived tokens were leaked via a compromised laptop. Rotating every "
            "token is the only way to guarantee the blast radius is contained. 2-FA prevents "
            "credential reuse even if another laptop is compromised. Disruption to active "
            "integrations is acceptable given the security risk."
        ),
        topic_keys=["security"],
        options=["Rotate all tokens + enforce 2-FA immediately", "Rotate only the 3 affected tokens + 2-week 2-FA rollout"],
        chose="Rotate all tokens + enforce 2-FA immediately",
        hypotheses=[ids["hy-002"]],
        evidence=[ids["ev-004"]],
    ))
    accept("human:sarah-l", ids["d-008"])
    accept("human:dan-r", ids["d-008"])
    disagree(
        "human:alex-m", ids["d-008"],
        reason=(
            "Token rotation will break active integrations for 12+ enterprise customers "
            "mid-billing-cycle. Prefer to require rotation only for the 3 affected tokens "
            "and roll out 2-FA with a 2-week notice period."
        ),
    )

    register("d-009", emit_decision(
        "human:sarah-l",
        title="Disable API-key authentication; require JWT for all API access",
        rationale=(
            "API keys are static secrets with no expiry. Post-incident analysis shows "
            "long-lived keys are the primary risk surface. JWT tokens expire in 24h and "
            "are tied to a session. Deprecating API keys closes that attack vector permanently."
        ),
        topic_keys=["security"],
        options=["Disable API keys (JWT-only)", "Keep API keys with mandatory 90-day expiry", "Keep API keys unchanged"],
        chose="Disable API keys (JWT-only)",
    ))
    accept("human:sarah-l", ids["d-009"])
    accept("agent:claude:sec", ids["d-009"])
    disagree(
        "human:priya-k", ids["d-009"],
        reason=(
            "API keys are required by 7 integrations that cannot rotate to JWT in the "
            "proposed 2-week window. A deprecation timeline of 6 months with migration "
            "tooling is required."
        ),
    )

    print("\n--- Chapter 4: Auth Supersession Chain D-003 → D-010 → D-011 ---")

    register("d-010", emit_decision(
        "human:alex-m",
        title="Migrate API authentication to WorkOS AuthKit",
        rationale=(
            "Load testing at 520 concurrent users revealed JWT + in-memory cache causes "
            "8% 401 errors due to cache eviction races (hypothesis hy-001 refuted). Adding "
            "Redis eliminates errors but doubles auth latency. WorkOS AuthKit at 280 ms p99 "
            "at 1000 concurrent is superior. Post-Series-A budget now justifies the $24K/year "
            "cost. Supersedes the original JWT approach."
        ),
        topic_keys=["security", "infrastructure"],
        options=["WorkOS AuthKit", "Redis-backed JWT", "Auth0"],
        chose="WorkOS AuthKit",
        hypotheses=[ids["hy-001"]],
        evidence=[ids["ev-009"]],
    ))
    accept("human:priya-k", ids["d-010"])
    accept("human:sarah-l", ids["d-010"])
    supersede(ids["d-003"], ids["d-010"])

    register("d-011", emit_decision(
        "human:priya-k",
        title="Add SSO fallback provider (Okta) alongside WorkOS for auth resilience",
        rationale=(
            "WorkOS experienced a 47-minute outage on 2026-07-03 that caused 12% login "
            "failures. Single auth provider is a critical availability risk. Adding Okta "
            "as a hot standby allows failover in under 5 minutes. Supersedes WorkOS-only "
            "approach."
        ),
        topic_keys=["security", "infrastructure"],
        options=["WorkOS + Okta hot standby", "WorkOS only (accept risk)", "Build in-house SSO abstraction layer"],
        chose="WorkOS + Okta hot standby",
        evidence=[ids["ev-006"]],
    ))
    accept("human:alex-m", ids["d-011"])
    accept("human:tom-b", ids["d-011"])
    supersede(ids["d-010"], ids["d-011"])

    print("\n--- Chapter 4b: DB Supersession Chain D-001 → D-012 → D-013 ---")

    register("d-012", emit_decision(
        "human:priya-k",
        title="Add a Postgres read replica for dashboard and analytics queries",
        rationale=(
            "Dashboard queries now account for 35% of DB load and are causing p95 latency "
            "degradation on write paths. A read replica isolates read load. This is the first "
            "step on the DB scaling ladder; connection pooling and sharding are deferred."
        ),
        topic_keys=["infrastructure"],
        options=["Postgres primary + read replica", "Read-through cache (Redis)"],
        chose="Postgres primary + read replica",
    ))
    accept("human:alex-m", ids["d-012"])
    supersede(ids["d-001"], ids["d-012"])

    register("d-013", emit_decision(
        "human:priya-k",
        title="Add PgBouncer connection pooler in front of Postgres (primary + replica)",
        rationale=(
            "After read replica deploy, we still see connection exhaustion during peak "
            "(>180 connections). PgBouncer in transaction-pooling mode lets us serve 1000+ "
            "app connections over 50 backend connections. Eliminates connection exhaustion "
            "without schema changes."
        ),
        topic_keys=["infrastructure"],
        options=["PgBouncer transaction mode", "RDS Proxy (AWS managed)", "Shard the database"],
        chose="PgBouncer transaction mode",
    ))
    accept("human:alex-m", ids["d-013"])
    accept("agent:claude:arch", ids["d-013"])
    supersede(ids["d-012"], ids["d-013"])

    print("\n--- Chapter 5: Product Pivot (Aug-Oct 2026) ---")

    register("d-014", emit_decision(
        "human:tom-b",
        title="Do not add cryptocurrency payment support in 2026",
        rationale=(
            "Crypto volume in B2B e-commerce is < 0.3% of GMV per Stripe data. Compliance "
            "burden (FinCEN MSB registration, BitLicense NY) is disproportionate. Engineering "
            "cost is 3+ months. Deferred to 2027 reassessment when regulatory clarity improves."
        ),
        topic_keys=["product", "payments"],
        options=["Add crypto support (BTC + ETH)", "Defer to 2027"],
        chose="Defer to 2027",
    ))
    accept("human:dan-r", ids["d-014"])
    accept("human:sarah-l", ids["d-014"])

    register("d-015", emit_decision(
        "human:tom-b",
        title="Expand product to EU market (starting with Germany + Netherlands)",
        rationale=(
            "38% of surveyed platforms are expanding to EU within 12 months. iDEAL (NL) "
            "and SOFORT (DE) are top-requested payment methods. Stripe supports both natively. "
            "We target Q1 2027 GA; compliance groundwork (GDPR, local payment methods) "
            "starts Q4 2026."
        ),
        topic_keys=["product", "payments"],
        options=["EU expansion (DE + NL first)", "APAC expansion", "Deepen US coverage only"],
        chose="EU expansion (DE + NL first)",
        evidence=[ids["ev-005"]],
    ))
    accept("human:dan-r", ids["d-015"])
    accept("human:priya-k", ids["d-015"])

    register("d-016", emit_decision(
        "human:priya-k",
        title="Deprecate Webhook API v1; require migration to Webhook API v2",
        rationale=(
            "Webhook v1 lacks idempotency keys, has no retry logic, and exposes raw event "
            "payloads that caused two data-consistency incidents in Q2. v2 fixes all three. "
            "We will give customers 6 months (until 2027-04-01) to migrate. v1 will be "
            "disabled, not sunset slowly, to avoid split maintenance burden."
        ),
        topic_keys=["product", "engineering"],
        options=["Hard deprecate v1 with 6-month window", "Keep v1 indefinitely (long-term maintenance)", "Immediate shutdown (1-month notice)"],
        chose="Hard deprecate v1 with 6-month window",
    ))
    accept("human:dan-r", ids["d-016"])
    accept("human:alex-m", ids["d-016"])

    register("d-017", emit_decision(
        "human:sarah-l",
        title="Host EU customer data exclusively in AWS eu-central-1 (Frankfurt)",
        rationale=(
            "GDPR data residency requires EU personal data stay in the EU or an adequacy-listed "
            "country. eu-central-1 (Frankfurt) and eu-west-1 (Dublin) both qualify. Frankfurt "
            "has lower latency to our primary DE + NL markets (13 ms vs 22 ms from Dublin). "
            "All EU tenants get an isolated RDS instance in eu-central-1; no cross-region "
            "replication of personal data."
        ),
        topic_keys=["infrastructure", "security", "payments"],
        options=["eu-central-1 (Frankfurt)", "eu-west-1 (Dublin)"],
        chose="eu-central-1 (Frankfurt)",
        evidence=[ids["ev-007"]],
    ))
    accept("human:tom-b", ids["d-017"])
    accept("human:alex-m", ids["d-017"])

    print("\n--- Chapter 6: Compliance & Scale (Oct-Dec 2026) ---")

    register("d-018", emit_decision(
        "human:tom-b",
        title="Appoint external DPO firm for GDPR compliance",
        rationale=(
            "GDPR Article 37 mandates a Data Protection Officer when processing large-scale "
            "sensitive data. In-house DPO would cost ~$180K/year (loaded) plus 3-6 months to "
            "hire. External DPO firm at $36K/year provides immediate coverage, regulatory "
            "network, and scales with headcount. Selected firm: DataGuard (EU-specialist)."
        ),
        topic_keys=["security", "product"],
        options=["External DPO firm (DataGuard)", "In-house DPO hire"],
        chose="External DPO firm (DataGuard)",
        evidence=[ids["ev-007"]],
    ))
    accept("human:sarah-l", ids["d-018"])
    accept("human:dan-r", ids["d-018"])

    register("d-019", emit_decision(
        "human:sarah-l",
        title="Upgrade to PCI DSS Level 1 compliance ahead of EU launch",
        rationale=(
            "EU enterprise customers require PCI Level 1 (Report on Compliance, not "
            "self-assessment). Current SAQ-A is insufficient for contracts above €1M GMV/year. "
            "Level 1 audit cost ($120K) is offset by unlocking the enterprise tier. Audit "
            "scope: Stripe integration, tokenisation flow, audit logging."
        ),
        topic_keys=["security", "payments"],
        options=["Upgrade to PCI DSS Level 1 (QSA audit)", "Remain at SAQ-A (defer enterprise EU tier)", "Pursue PCI DSS Level 2 (SAQ-D)"],
        chose="Upgrade to PCI DSS Level 1 (QSA audit)",
        evidence=[ids["ev-003"]],
    ))
    accept("human:tom-b", ids["d-019"])
    accept("human:alex-m", ids["d-019"])

    register("d-020", emit_decision(
        "human:alex-m",
        title="Reorganise engineering into a Platform team and two Product teams",
        rationale=(
            "Single-team structure creates coordination overhead as we approach 20 engineers. "
            "Platform team owns: payments infra, auth, DB, deployment. Product teams (2) own: "
            "Merchant Dashboard and Partner API. Avoids a third-tier hierarchy that would slow "
            "decisions."
        ),
        topic_keys=["engineering"],
        options=["Platform + 2 product teams", "Feature squads (no platform)", "Functional departments (Eng / Product / Infra)"],
        chose="Platform + 2 product teams",
    ))
    accept("human:tom-b", ids["d-020"])
    accept("human:priya-k", ids["d-020"])

    register("d-021", emit_decision(
        "human:dan-r",
        title="Move merchant onboarding to self-serve (remove white-glove prerequisite)",
        rationale=(
            "White-glove onboarding caps throughput at 8 new merchants/month (3 sales engineers, "
            "full-day sessions). Self-serve with docs + sandbox targets 50+ merchants/month. "
            "B2B SaaS comps show self-serve achieves comparable activation rates when sandbox "
            "is frictionless. Requires: improved CLI quickstart, hosted sandbox env, video "
            "walkthroughs."
        ),
        topic_keys=["product", "engineering"],
        options=["Self-serve + async support", "White-glove only (status quo)", "Hybrid (self-serve + optional white-glove)"],
        chose="Self-serve + async support",
    ))
    accept("human:tom-b", ids["d-021"])
    accept("human:alex-m", ids["d-021"])

    register("d-022", emit_decision(
        "human:alex-m",
        title="Adopt Temporal.io for all long-running payment workflow orchestration",
        rationale=(
            "Payout workflows, dispute handling, and reconciliation jobs all need durable "
            "execution with retries. Current approach (SQS + Lambda + ad-hoc state in Postgres) "
            "has caused 4 data-consistency incidents in 2026. Temporal provides durable saga "
            "patterns, built-in replay, and a workflow history UI. Replaces ad-hoc queue/cron "
            "approach."
        ),
        topic_keys=["infrastructure", "payments", "engineering"],
        options=["Temporal.io (self-hosted)", "AWS Step Functions", "Maintain current SQS+Lambda approach"],
        chose="Temporal.io (self-hosted)",
    ))
    accept("human:priya-k", ids["d-022"])
    accept("agent:claude:arch", ids["d-022"])

    register("d-023", emit_decision(
        "human:sarah-l",
        title="Freeze EU beta data in us-east-1; defer eu-central-1 migration to GA",
        rationale=(
            "Early EU beta (3 pilot customers) showed data volumes too small to justify a "
            "dedicated eu-central-1 instance. Legal confirmed pilot data is covered by SCCs "
            "under our existing DPA. We will revisit at 100+ EU tenants. Supersedes d-017 "
            "for beta phase only; d-017 remains the target for GA."
        ),
        topic_keys=["infrastructure", "security"],
        options=["Keep beta data in us-east-1 (defer eu-central-1)", "Provision eu-central-1 even for beta"],
        chose="Keep beta data in us-east-1 (defer eu-central-1)",
    ))
    accept("human:tom-b", ids["d-023"])
    disagree(
        "human:dan-r", ids["d-023"],
        reason=(
            "Freezing EU data in us-east-1, even temporarily, creates a compliance narrative "
            "risk if leaked to press or prospects. Prefer a minimal eu-central-1 RDS instance "
            "even for beta."
        ),
    )

    register("d-024", emit_decision(
        "human:priya-k",
        title="Use Sentry for error monitoring and Datadog for infrastructure metrics",
        rationale=(
            "Two tools rather than one: Sentry owns application errors (stack traces, release "
            "tracking, user context); Datadog owns infra metrics, APM, and logs. Evaluated "
            "New Relic (all-in-one) — worse DX for Rust, higher cost at our scale. Combined "
            "Sentry+Datadog is $18K/year vs New Relic $22K."
        ),
        topic_keys=["engineering", "infrastructure"],
        options=["Sentry + Datadog", "New Relic (all-in-one)", "Grafana OSS stack (self-hosted)"],
        chose="Sentry + Datadog",
    ))
    accept("human:alex-m", ids["d-024"])

    register("d-025", emit_decision(
        "human:priya-k",
        title="Adopt LaunchDarkly for all new feature rollouts",
        rationale=(
            "EU expansion and PCI Level 1 rollout both need progressive deployment with instant "
            "kill-switches. Current approach (hard-coded env vars) cannot target by tenant or "
            "percentage. LaunchDarkly at $12K/year with SDK support for Rust. Custom "
            "implementation estimated at 6 weeks."
        ),
        topic_keys=["engineering", "product"],
        options=["LaunchDarkly", "Build in-house flag service", "Unleash OSS (self-hosted)"],
        chose="LaunchDarkly",
    ))
    accept("human:dan-r", ids["d-025"])
    accept("human:alex-m", ids["d-025"])

    # ── Semantic relations ───────────────────────────────────────────────────
    # Note: the CLI's relation.added only supports evidence↔hypothesis and
    # evidence→decision links (supports/refutes/based-on).
    # Decision→decision semantic links (SUPPORTS, ASSUMES) are not yet in
    # the CLI — they require the capture/ingest path or a future CLI command.
    # The corpus.yaml documents the intended decision→decision links for
    # reference and future loading.
    print("\n--- Evidence/hypothesis relations ---")

    # ev-009 (auth scale test) REFUTES hy-001 (JWT scales to 1000 users)
    add_relation("refutes", ids["ev-009"], ids["hy-001"])

    # ev-007 (GDPR counsel) REFUTES hy-003 (EU needs no GDPR audit)
    add_relation("refutes", ids["ev-007"], ids["hy-003"])

    # ev-004 (incident report confirms isolation) SUPPORTS hy-002
    add_relation("supports", ids["ev-004"], ids["hy-002"])

    # Extra evidence attachments (cross-reference beyond initial decision.proposed links)
    # ev-006 (WorkOS outage) also supports d-011 rationale directly
    attach_evidence(ids["d-011"], ids["ev-006"])

    # ev-003 (SAQ-A legal memo) also supports d-019 (PCI Level 1 upgrade context)
    attach_evidence(ids["d-019"], ids["ev-003"])

    # ── Scorer events (optional) ─────────────────────────────────────────────
    if EMIT_SCORES:
        print("\n--- Scorer events ---")
        print("NOTE: Requires hivemind-m2-shared-backend-lives-uuq9.19 to be merged")
        # scorer events would be emitted here via decision.scored command
        print("(scorer events not yet emittable via CLI — see corpus.yaml for authored scores)")

    # ── Summary ──────────────────────────────────────────────────────────────
    print(f"\n=== Load complete ===")
    print(f"Loaded {len([k for k in ids if k.startswith('d-')])} decisions, "
          f"{len([k for k in ids if k.startswith('ev-')])} evidence nodes, "
          f"{len([k for k in ids if k.startswith('hy-')])} hypotheses.")
    print(f"\nVerify with:")
    print(f"  {HM} --hivemind-dir {HIVEMIND_DIR} --tenant {TENANT} query decisions --json | python3 -c 'import json,sys; d=json.load(sys.stdin); print(len(d), \"decisions\")'")
    print(f"\nID map saved to: demo-id-map.txt")
    with open("demo-id-map.txt", "w") as f:
        for k, v in ids.items():
            f.write(f"{k}\t{v}\n")


if __name__ == "__main__":
    main()
