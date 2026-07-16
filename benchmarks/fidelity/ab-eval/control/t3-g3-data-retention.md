# Source Document Bundle — Data Lifecycle Policy
# G3: 3 documents, temporal gaps (Feb→May→Sep), terminology drift, zero cross-references

---

## Document 1: Legal & Compliance Memo (2024-02-05)

Subject: Statutory Record Obligations — Updated Guidance

Following the January regulatory update, our statutory obligations require
retaining all customer transaction records for a minimum of seven years.
This supersedes the prior informal three-year practice. Engineering teams
should align their storage architecture accordingly before end of Q2.

---

## Document 2: Infrastructure Cost Proposal (2024-05-14)

To reduce monthly object storage expenditure, we propose the following
data lifecycle policy:

  Tier 1 (active): 90-day retention in standard storage
  Tier 2 (warm): 180-day cold archive, access latency ≤24h
  Tier 3 (close): account data purged 365 days after contract end

Estimated savings: ~40% reduction in monthly storage cost. Proposal
approved by engineering leadership; pending infrastructure review.

---

## Document 3: Customer Success Escalation Note (2024-09-03)

Escalation: Meridian Corp — record access SLA breach

Meridian Corp's master services agreement guarantees sixty-month online
access to all historical transaction data. A recent request for 22-month-old
records triggered automatic warm-tier retrieval (24-hour delay), violating
the contract's "next business day" access SLA.

This is the fourth similar case this quarter from enterprise accounts with
multi-year contract terms. The enterprise team requests a policy exception
for accounts on multi-year MSAs.
