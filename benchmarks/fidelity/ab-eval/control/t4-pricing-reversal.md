# ADR: Product Pricing Model (Reversal)

**Status**: Superseded (usage-based → flat per-seat)
**Domain**: Finance / Product

---

## Decision 1 (Original, March): Usage-Based Pricing

**Status**: Superseded by Decision 2

Usage-based pricing: customers pay per unit of usage.

---

## Decision 2 (Current): Flat Per-Seat Pricing

**Status**: Accepted
**Supersedes**: Decision 1 (usage-based pricing)

**Chosen: flat per-seat pricing.**

### Rationale for Reversal

Usage-based pricing caused bill shock for customers with bursty usage
patterns. Q2 churn rose by 4 percentage points (compared to prior
quarter) — directly attributed to bill shock from the usage-based model.

Flat per-seat pricing:
- Eliminates bill surprise for customers
- Simpler for sales to explain
- Predictable revenue for the company

### Evidence

- Q2 churn data: usage-based pricing period saw churn increase by 4 points
  above the prior period (bill shock attributed in exit interviews).

## Consequences

- Per-seat pricing deployed immediately.
- All existing usage-based contracts migrated.
- Pricing page updated.
