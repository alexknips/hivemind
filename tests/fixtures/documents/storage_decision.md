# Storage decision notes

Decision:
  id: local-storage
  title: Use SQLite for the local prototype
  status: accepted
  actor: actor:alice
  topic_keys: storage, local
  rationale: Keeps onboarding under five minutes while preserving replayable local state.
  options:
    - sqlite
    - postgres
  chose: sqlite
  evidence:
    - Local replay tests complete in under one second.
  hypotheses:
    - Embedded storage is enough for the single-user slice.
