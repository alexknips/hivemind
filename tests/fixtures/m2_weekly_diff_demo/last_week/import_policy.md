# Last week notes

These notes captured a policy decision made before this demo's diff window.
The diff query for the new window must not include this decision under either
`added_decisions` or `changed_existing_decisions`.

Decision:
  id: last-week-import-policy
  title: Treat document import as deterministic
  status: accepted
  actor: actor:alice
  topic_keys: ingestion, governance
  rationale: Slack ingestion already proved that deterministic provenance is essential.
  options:
    - deterministic
    - heuristic
  chose: deterministic
  evidence:
    - Deterministic markers can be reviewed before they hit the ledger.
  hypotheses:
    - Provenance gaps will surface before agents act on imported decisions.
