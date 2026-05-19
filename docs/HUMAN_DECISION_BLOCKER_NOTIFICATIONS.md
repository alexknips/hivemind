# Human Decision Blocker Notifications

Status: research recommendation for human and agent blocker notifications.

Question: when should HiveMind notify a human that an actor is blocked on a
decision, and what should the notification contain?

## Recommendation

Add deterministic blocker-notification queries over explicit ledger events. A
human should be notified when a blocker is:

- explicitly reported by an actor;
- tied to a decision state that cannot advance deterministically;
- old enough for its priority;
- blocking enough downstream actors or external work references;
- owned by a human authority that an agent must not replace.

This is not task tracking. HiveMind should store decision blockers as decision
memory facts: who is blocked, which decision or hypothesis is blocking them, why
the decision cannot be made yet, what external work reference is affected, and
what prior notifications were already sent.

Notification eligibility belongs in the query/read layer when it is a
deterministic derivation from explicit events and policy. Ranking, rewritten
summaries, similar-blocker clustering, and suggested recipients belong in the
agentic layer and must remain optional.

## Blocked-On-Decision States

| State | Meaning | First deterministic signal |
| --- | --- | --- |
| Explicit request | An actor says work cannot proceed without a decision. | `decision.requested` or `blocker.reported` with actor, topic or decision, priority, blocked reference, and requested owner. |
| Unresolved options | A decision has options but no accepted choice, and an actor marks work as waiting on that choice. | Decision is `proposed` or `contested`, has `HAS_OPTION` edges, lacks an accepted outcome, and has an active blocker edge. |
| Open evidence conflict | Evidence or hypotheses still conflict with the decision. | A decision `ASSUMES` a hypothesis that is `contested` or `refuted`, or visible evidence both `SUPPORTS` and `REFUTES` a required hypothesis. |
| Missing owner | The required approver, domain owner, or accountable actor is absent or unknown. | Blocker payload has `required_owner_id` missing, unknown, inactive, or unauthorized for the authority class. |
| SLA exceeded | The blocker or decision has exceeded the response window for its priority. | `now - blocker_reported_at` or `now - last_progress_at` crosses the configured threshold. |
| Repeated bounce | Work keeps returning to the same decision without resolution. | Three or more blocker reports or external-work bounce correlations for the same decision or topic in a suppression window. |
| Human-only authority | A decision requires human judgment, approval, legal/security authority, customer commitment, budget approval, or people impact. | `authority_class=human_required` or a matching owner policy on topic or decision type. |

`contested` is a real blocker state when another actor depends on the decision.
It must be surfaced as disagreement, not converted into a generic pending state.

## Threshold Matrix

Thresholds use the highest applicable row. "Direct" means Slack, email, message,
or another active channel to the responsible human. "Queue" means a dashboard or
escalation queue entry. "Digest" means a grouped low-urgency summary.

| Trigger | P0 critical | P1 high | P2 medium | P3/P4 low |
| --- | --- | --- | --- | --- |
| Explicit human-required blocker | Direct immediately; repeat after 15 min if unacknowledged. | Direct within 15 min; repeat after 2 h. | Queue immediately; direct after 4 business h. | Digest; direct only after 2 business days. |
| Missing required owner | Direct to escalation owner immediately. | Queue immediately; direct after 30 min. | Queue after 2 business h. | Digest after 1 business day. |
| Contested decision blocks work | Queue immediately; direct after 15 min. | Queue immediately; direct after 1 h. | Queue after 4 business h; direct after 1 business day. | Digest only. |
| Refuted assumption blocks accepted decision | Direct immediately for affected owners. | Queue immediately; direct after 30 min. | Queue after 2 business h. | Digest after 1 business day. |
| Unresolved options, no contest | Queue after 15 min; direct after 30 min. | Queue after 1 h; direct after 4 h. | Queue after 1 business day. | Digest after 2 business days. |
| Downstream blocked breadth | Direct when at least 2 actors or 3 external work refs are blocked. | Direct when at least 2 actors or 5 external work refs are blocked. | Queue when at least 3 actors or 8 external work refs are blocked. | Digest when at least 10 external work refs are blocked. |
| Repeated bounce | Direct on third bounce in 1 h. | Direct on third bounce in 1 business day. | Queue on third bounce in 2 business days. | Digest on fifth bounce in 5 business days. |

Default priority comes from the blocker report. If absent, use the linked
decision priority if one exists; otherwise use P2. Authority policy can raise
notification urgency, but it should not lower urgency below the explicit blocker
priority.

## Notification Surfaces

| Surface | Use | Do not use for |
| --- | --- | --- |
| Dashboard or UI badge | Persistent queue of active blockers, grouped by decision, owner, status, and affected actors. | Urgent P0/P1 human-required decisions that need immediate attention. |
| Read-only history agent response | User-initiated questions like "what is blocked and why?" with citations to events. | Pushing unsolicited action notifications or deciding who should approve. |
| Direct Slack, email, or message | P0/P1 or human-required blockers where a named human can unblock work. | Low-priority visibility, duplicate reminders, or unresolved ownership discovery. |
| Daily digest or escalation queue | Low-priority blockers, unresolved ownership, repeated stale summaries, and coordinator review. | Anything whose SLA is already breached at P0/P1. |

Direct notifications should be terse and actionable:

```text
Decision blocker: Agent buildbot is blocked on D-184 "Choose rollback vs hotfix".
Why: accepted incident decision assumes H-77, now refuted by E-231.
Impact: 2 actors and 4 work refs are blocked.
Needed from: oncall-lead by 2026-05-19T11:30:00Z.
Sources: event 812 blocker.reported, event 819 hypothesis.refuted.
```

The read-only history agent may answer the same question in richer prose, but it
must stay informational: cite events, explain current status, and avoid choosing
an option, assigning authority, or marking a blocker resolved.

## Anti-Spam Rules

- Dedupe by `(tenant, decision_id or topic_key, blocker_state, blocked_actor,
  required_owner, severity)` within the suppression window.
- Send at most one direct notification per owner per decision per suppression
  window, unless priority increases, blocked breadth doubles, or a new refuted
  assumption affects an accepted decision.
- Suppression windows: P0 15 minutes, P1 2 hours, P2 1 business day, P3/P4 3
  business days.
- Acknowledgement pauses repeats until the next SLA threshold or material state
  change.
- Resolution closes the active notification thread but keeps the event history.
- Escalation to a backup owner is allowed only after an explicit policy or owner
  roster event identifies the backup.
- Notification records are ledger events. The system must be able to answer who
  was notified, when, why, and from which source events.
- Queries that hit limits must return `truncated: true` and a continuation
  cursor. Never send a partial blocker digest that looks complete.

## Minimal Data And Query Model

Minimum write events:

| Event | Required fields |
| --- | --- |
| `decision.requested` | `actor_id`, `topic_keys`, optional `decision_id`, `reason`, `priority`, `required_owner_id`, `authority_class`, `requested_by`, `source_ref`, `client_request_id`. |
| `blocker.reported` | `blocked_actor_id`, `decision_id` or `topic_keys`, `blocked_ref`, `blocked_ref_type`, `reason`, `priority`, `last_progress_at`, `required_owner_id`, `source_ref`, `correlation_id`. |
| `blocker.resolved` | `blocker_id`, `actor_id`, `resolution_event_id` or `resolution_reason`, `source_ref`. |
| `notification.sent` | `blocker_id`, `recipient_actor_id`, `channel`, `threshold_rule`, `source_event_ids`, `dedupe_key`, `sent_at`. |
| `notification.acknowledged` | `notification_id`, `actor_id`, `ack_at`, optional `snooze_until`. |
| `owner.policy.recorded` | `topic_pattern` or `authority_class`, `owner_actor_id`, backup or escalation actor, and effective interval. |

The first implementation can start with `decision.requested`,
`blocker.reported`, and `notification.sent`, but the schema should not preclude
acknowledgements, snoozes, owner-policy updates, or later notification channels.

Minimum read/query APIs:

- `get_active_decision_blockers(filters, limit, cursor)` returns active blockers
  with decision status, stale/refuted assumptions, required owner, blocked
  actors, blocked external refs, last progress timestamp, threshold rule,
  notification state, `truncated`, and `next_cursor`.
- `get_blocker_notification_candidates(now, policy_version, limit, cursor)`
  returns deterministic candidate notifications with dedupe keys and source
  event ids. This is a query; the sender appends `notification.sent` only after
  delivery succeeds.
- `get_decision_blocker_history(blocker_id)` returns the event trail: report,
  related decision/evidence/hypothesis events, prior notifications,
  acknowledgements, and resolution.

These queries should call only the read model. They should not call an LLM,
infer missing owners, or rank blockers semantically.

## Examples

### Agent Blocked By Human Decision

An implementation agent reports that it cannot continue a billing migration
because the pricing owner has not chosen between two customer-notice options.
The decision is `proposed`, has two `HAS_OPTION` edges, no accepted outcome, and
`authority_class=human_required` because it changes customer commitments.

Threshold result: a P1 human-required blocker gets a queue entry immediately and
a direct notification to the pricing owner within 15 minutes. The message cites
the blocker report, the proposed decision, the two options, and the blocked
agent/run reference. The agent is not asked to choose the option.

### Human Blocked By Agent Decision

A support lead is blocked writing customer guidance because a triage agent has
not accepted or rejected its severity classification for an incident. The
classification decision is `contested`: the scanner agent proposes high
severity, while the maintainer agent rejects that severity based on deployment
evidence.

Threshold result: the support lead's blocker makes the contested decision
actionable. A P1 queue item appears immediately. If unresolved after 1 hour, a
direct notification goes to the incident owner showing the disagreement and the
events behind both positions. The notification asks for resolution; it does not
collapse the disagreement.

### Refuted Hypothesis Invalidates Accepted Work

A rollout decision was accepted assuming "regional failover is already tested".
Later evidence refutes that hypothesis. Two deployment agents and a human
release manager have active blockers linked to the rollout decision.

Threshold result: P0/P1 direct notification is immediate because an accepted
decision now rests on a refuted assumption and multiple actors are blocked. The
message points to the accepted decision, `ASSUMES` edge, refuting evidence, and
affected blocker reports.

## Relation To Adjacent Studies

The human ledger query experience is the pull surface: humans ask what happened,
what changed, and what is blocked. Decision blocker notification is the push
surface: HiveMind interrupts only when deterministic thresholds say a human can
unblock decision progress. Both surfaces need the same read-model facts,
citations, actor/source provenance, stable filters, pagination, and `truncated`
honesty.

The event granularity study recommends compound client commands that decompose
into canonical events. Blocker capture should follow the same rule: Slack,
agent, or UI clients may submit one ergonomic "I am blocked on this decision"
command, but the ledger should store canonical blocker, relation, and
notification events with `event_origin`, `actor_id`, `correlation_id`, and
`source_ref`.

## Open Product Decisions

- Exact priority vocabulary: reuse external work priority, add a HiveMind
  blocker severity, or derive both from policy.
- Whether `decision.requested` is distinct from `blocker.reported` in slice 2 or
  a convenience command that decomposes to blocker plus relation events.
- How owner policy is administered for topic patterns and authority classes.
- Which direct channel ships first: Slack, email, in-product notification, or
  the existing Gas City mail/runtime path for agent-only deployments.
- Whether business-hour SLA clocks are tenant-configured or fixed initially.
- How much blocker summary text may be generated by layer 3 before direct send;
  deterministic source events must remain visible either way.
- Whether notification acknowledgement can happen from outside HiveMind, for
  example Slack button actions, and how that maps to actor identity.
