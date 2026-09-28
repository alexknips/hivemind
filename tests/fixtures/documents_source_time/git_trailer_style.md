# Commit history, imported as decision text

A git trailer import is text extracted from commit metadata (subject, body,
trailers) and pasted into a decision block ahead of `import documents`. The
importer has no special-case code for "this text came from a commit" — the
`ts:` marker is what carries the commit's own authored time, same as any
other document source.

Decision:
  id: git-commit-storage-backend
  title: Use SQLite for the local ledger, per commit abc1234
  status: accepted
  actor: actor:alice
  ts: 2026-04-02T09:15:00Z
  topic_keys: storage, git-history
  rationale: The commit message explains the tradeoff; this block preserves it as a decision.
  options:
    - sqlite
    - postgres
  chose: sqlite
  evidence:
    - Commit abc1234's message: "sqlite needs no server process for the local prototype".
