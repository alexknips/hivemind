# This week notes

These notes capture a decision the demo expects to surface as added in the
diff window that starts after the last-week import.

Decision:
  id: weekly-cache-eviction
  title: Evict cached projections after each import run
  status: proposed
  actor: actor:bob
  topic_keys: caching, performance
  rationale: Stale projection caches masked import duplicates in earlier slices.
  options:
    - evict-on-import
    - evict-nightly
  chose: evict-on-import
  evidence:
    - Local replay tests highlighted duplicate decisions only after eviction.
  hypotheses:
    - Eager eviction will keep weekly diffs aligned with the ledger.
