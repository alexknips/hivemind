# Tracker ticket, imported as decision text

A tracker/beads ticket import is the ticket's body pasted into a decision
block ahead of `import documents` — the same generic document importer as
any other text source. The `ts:` marker carries the ticket's own resolved
time; there is no ask recorded (a closed ticket records that something was
decided, not when it was first asked).

Decision:
  id: tracker-ticket-hivemind-ex01
  title: Ship the weekly digest as email, not a Slack DM
  status: accepted
  actor: actor:bob
  ts: 2026-04-05T16:30:00Z
  topic_keys: digest, tracker
  rationale: The ticket's resolution comment explains the choice; this block preserves it.
  options:
    - email
    - slack-dm
  chose: email
  evidence:
    - Ticket resolution comment: "email survives a channel archive; a DM does not."
