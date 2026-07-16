# Source Document Bundle — Frontend Framework Evaluation
# G2: 2 documents, different authors, no shared actor names, no cross-references

---

## Document 1: Product Engineering Planning Notes — Q1 2025 (2025-01-15)

**Objective**: Rebuild the customer-facing analytics dashboard.

The team evaluated several options for the frontend framework. Maya has
put forward SvelteKit as the preferred choice: smaller bundle size,
faster initial page load, and a simpler component model that the team
felt would speed up development.

Infrastructure review was still in progress. Dashboard work was scheduled
to begin after framework selection, expected end of Q1.

---

## Document 2: Engineering Retrospective — Q1 2025 (2025-03-28)

**Highlights and Blockers**

Build tooling migration to Vite completed successfully.

Dashboard rebuild: stalled in Q1. Chris (DevOps) flagged late in the
quarter that SvelteKit's server-side rendering model would require new
Kubernetes deployment configuration not budgeted for Q1. He recommended
staying with React, which already has mature operator tooling in our
cluster. No final framework selection was made before Q1 ended. Carried
into Q2 planning.
