// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use std::cell::Cell;

use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::commands::{
    Commands, DecisionProposalInput, DeterminedProject, Grounding, SupersedeInput,
};
use crate::events::{Event, EventId, EventSource, EventType, ProjectLinkKind};
use crate::ledger::{EventLedger, InMemoryEventLedger};
use crate::projector::{
    memory::MemoryGraph, rebuild_graph, GraphParams, GraphProperties, GraphRow, GraphView,
    NodeKind, RelationKind,
};
use crate::Result;

/// A small ledger builder for tests about what decisions rest on: each call appends one event
/// with a fixed, increasing timestamp, so the graph a test reads is exactly the events it wrote.
///
/// Capture-time premise links carry `causation_event_id` = the proposal event, the same way
/// `Commands::propose_decision` fans them out; links added later carry none.
pub(crate) struct Scenario {
    ledger: InMemoryEventLedger,
    sequence: Cell<u128>,
}

pub(crate) fn ts(timestamp: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(timestamp)
        .expect("test timestamp parses")
        .with_timezone(&Utc)
}

impl Scenario {
    pub(crate) fn new() -> Self {
        Self {
            ledger: InMemoryEventLedger::new(),
            sequence: Cell::new(0),
        }
    }

    pub(crate) fn ledger(&self) -> &InMemoryEventLedger {
        &self.ledger
    }

    pub(crate) fn graph(&self) -> Result<MemoryGraph> {
        let graph = MemoryGraph::default();
        rebuild_graph(&self.ledger, &graph)?;
        Ok(graph)
    }

    fn push(
        &self,
        actor_id: &str,
        event_type: EventType,
        payload: Value,
        causation_event_id: Option<EventId>,
        timestamp: &str,
    ) -> Result<EventId> {
        self.push_with_source_ref(
            actor_id,
            event_type,
            payload,
            causation_event_id,
            timestamp,
            None,
        )
    }

    /// `push` for the event types that require a `source_ref`.
    fn push_with_source_ref(
        &self,
        actor_id: &str,
        event_type: EventType,
        payload: Value,
        causation_event_id: Option<EventId>,
        timestamp: &str,
        source_ref: Option<&str>,
    ) -> Result<EventId> {
        let sequence = self.sequence.get() + 1;
        self.sequence.set(sequence);
        self.ledger.append(Event {
            tenant_id: Default::default(),
            event_id: None,
            event_uuid: Uuid::from_u128(sequence),
            correlation_id: Some("grounding-test".to_owned()),
            causation_event_id,
            event_type,
            actor_id: actor_id.to_owned(),
            source: EventSource::Cli,
            source_ref: source_ref.map(str::to_owned),
            payload,
            ts: Some(ts(timestamp)),
        })
    }

    /// `decision.proposed` with no option, evidence or hypothesis.
    pub(crate) fn decision(
        &self,
        decision_id: &str,
        title: &str,
        actor_id: &str,
        timestamp: &str,
    ) -> Result<EventId> {
        self.decision_with(
            decision_id,
            title,
            actor_id,
            timestamp,
            false,
            &[],
            &[],
            None,
        )
    }

    /// `decision.proposed` with everything a capture can name. `chosen` picks an option, so
    /// hypotheses hang off it (the capture-time shape); without one they hang off the decision.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn decision_with(
        &self,
        decision_id: &str,
        title: &str,
        actor_id: &str,
        timestamp: &str,
        chosen: bool,
        evidence_ids: &[&str],
        hypothesis_ids: &[&str],
        expressed_confidence: Option<&str>,
    ) -> Result<EventId> {
        let option_id = format!("{decision_id}:opt:a");
        let mut payload = json!({
            "decision_id": decision_id,
            "title": title,
            "rationale": format!("Rationale for {title}, long enough to read on its own"),
            "topic_keys": ["grounding"],
            "option_ids": if chosen { vec![option_id.clone()] } else { Vec::new() },
            "option_labels": if chosen { vec!["Option A".to_owned()] } else { Vec::new() },
            "chosen_option_id": if chosen { Value::String(option_id) } else { Value::Null },
            "hypothesis_ids": hypothesis_ids,
            "evidence_ids": evidence_ids,
        });
        if let Some(confidence) = expressed_confidence {
            payload["expressed_confidence"] = json!(confidence);
        }
        self.push(
            actor_id,
            EventType::DecisionProposed,
            payload,
            None,
            timestamp,
        )
    }

    /// `decision.proposed` from a hand-built payload, for tests that control exactly which
    /// options, question and evidence a record carries (see `record_payload`).
    pub(crate) fn proposal(
        &self,
        actor_id: &str,
        timestamp: &str,
        payload: Value,
    ) -> Result<EventId> {
        self.push(
            actor_id,
            EventType::DecisionProposed,
            payload,
            None,
            timestamp,
        )
    }

    /// `decision.requested` naming `decision_id`, which nobody has proposed: the graph gets a
    /// bare stub node for it, with none of a proposal's properties.
    pub(crate) fn request_naming(&self, decision_id: &str, timestamp: &str) -> Result<EventId> {
        self.push_with_source_ref(
            "agent:tester",
            EventType::DecisionRequested,
            json!({
                "topic_keys": ["quality"],
                "decision_id": decision_id,
                "reason": "Somebody has to decide this",
                "priority": "P2",
                "authority_class": "team",
                "requested_by": "agent:tester",
                "client_request_id": format!("request-{decision_id}"),
            }),
            None,
            timestamp,
            Some("test:scenario"),
        )
    }

    pub(crate) fn evidence(
        &self,
        evidence_id: &str,
        content: &str,
        source: Option<&str>,
        timestamp: &str,
    ) -> Result<EventId> {
        let mut payload = json!({"evidence_id": evidence_id, "content": content});
        if let Some(source) = source {
            payload["source"] = json!(source);
        }
        self.push(
            "agent:tester",
            EventType::EvidenceRecorded,
            payload,
            None,
            timestamp,
        )
    }

    /// `hypothesis.recorded`; `kind` is `assumption` or `bet`.
    pub(crate) fn hypothesis(
        &self,
        hypothesis_id: &str,
        statement: &str,
        kind: &str,
        check_by: Option<&str>,
        timestamp: &str,
    ) -> Result<EventId> {
        let mut payload = json!({
            "hypothesis_id": hypothesis_id,
            "statement": statement,
            "kind": kind,
        });
        if let Some(check_by) = check_by {
            payload["check_by"] = json!(check_by);
        }
        self.push(
            "agent:tester",
            EventType::HypothesisRecorded,
            payload,
            None,
            timestamp,
        )
    }

    pub(crate) fn accept(&self, decision_id: &str, actor_id: &str, timestamp: &str) -> Result<()> {
        self.push(
            actor_id,
            EventType::DecisionAccepted,
            json!({"decision_id": decision_id}),
            None,
            timestamp,
        )
        .map(|_| ())
    }

    /// `decision.moved`: the decision leaves project `from` for project `to`.
    pub(crate) fn moved(
        &self,
        decision_id: &str,
        from: &str,
        to: &str,
        actor_id: &str,
        timestamp: &str,
    ) -> Result<()> {
        self.push(
            actor_id,
            EventType::DecisionMoved,
            json!({"decision_id": decision_id, "from": from, "to": to}),
            None,
            timestamp,
        )
        .map(|_| ())
    }

    pub(crate) fn reject(&self, decision_id: &str, actor_id: &str, timestamp: &str) -> Result<()> {
        self.push(
            actor_id,
            EventType::DecisionRejected,
            json!({"decision_id": decision_id}),
            None,
            timestamp,
        )
        .map(|_| ())
    }

    pub(crate) fn supersede(
        &self,
        old_decision_id: &str,
        new_decision_id: &str,
        actor_id: &str,
        timestamp: &str,
    ) -> Result<()> {
        self.push(
            actor_id,
            EventType::DecisionSuperseded,
            json!({"old_decision_id": old_decision_id, "new_decision_id": new_decision_id}),
            None,
            timestamp,
        )
        .map(|_| ())
    }

    /// `relation.added`. `causation` is the proposal event for a link named at capture, `None`
    /// for one attributed later.
    pub(crate) fn relation(
        &self,
        relation: &str,
        from_id: &str,
        to_id: &str,
        actor_id: &str,
        causation: Option<EventId>,
        timestamp: &str,
    ) -> Result<()> {
        self.push(
            actor_id,
            EventType::RelationAdded,
            json!({"relation": relation, "from_id": from_id, "to_id": to_id}),
            causation,
            timestamp,
        )
        .map(|_| ())
    }
}

/// A graph that counts the reads made through it, for tests of what a read costs.
pub(crate) struct CountingGraph<'a, G: GraphView> {
    inner: &'a G,
    queries: Cell<usize>,
}

impl<'a, G: GraphView> CountingGraph<'a, G> {
    pub(crate) fn new(inner: &'a G) -> Self {
        Self {
            inner,
            queries: Cell::new(0),
        }
    }

    /// How many `query` calls have gone through so far.
    pub(crate) fn queries(&self) -> usize {
        self.queries.get()
    }
}

impl<G: GraphView> GraphView for CountingGraph<'_, G> {
    fn upsert_node(&self, kind: NodeKind, id: &str, properties: &GraphProperties) -> Result<()> {
        self.inner.upsert_node(kind, id, properties)
    }

    fn upsert_edge(
        &self,
        kind: RelationKind,
        from_id: &str,
        to_id: &str,
        properties: &GraphProperties,
    ) -> Result<()> {
        self.inner.upsert_edge(kind, from_id, to_id, properties)
    }

    fn query(&self, cypher: &str, params: &GraphParams) -> Result<Vec<GraphRow>> {
        self.queries.set(self.queries.get() + 1);
        self.inner.query(cypher, params)
    }

    fn wipe(&self) -> Result<()> {
        self.inner.wipe()
    }
}

/// A `decision.proposed` payload the way a capture writes one. `options` are `(id, label,
/// description)` and `chosen` names one of them; a `question` comes with the `quote` it answers.
pub(crate) fn record_payload(
    decision_id: &str,
    question: Option<&str>,
    options: &[(&str, &str, &str)],
    chosen: Option<&str>,
    evidence_ids: &[&str],
) -> Value {
    let mut payload = json!({
        "decision_id": decision_id,
        "title": format!("Decision {decision_id}"),
        "rationale": format!("Why {decision_id} was taken, in the decider's words"),
        "topic_keys": ["quality"],
        "option_ids": options.iter().map(|(id, _, _)| *id).collect::<Vec<_>>(),
        "option_labels": options.iter().map(|(_, label, _)| *label).collect::<Vec<_>>(),
        "option_descriptions": options.iter().map(|(_, _, description)| *description).collect::<Vec<_>>(),
        "chosen_option_id": chosen,
        "evidence_ids": evidence_ids,
    });
    if let Some(question) = question {
        payload["question"] = json!(question);
        payload["quote"] = json!("Agreed in the review");
    }
    payload
}

/// Every decision `floor_scenario` records that a profile is read for, plus one id that is not in
/// the graph.
pub(crate) const FLOOR_SCENARIO_DECISIONS: [&str; 18] = [
    "d:solid",
    "d:placeholders",
    "d:bare",
    "d:later-evidence",
    "d:backfilled",
    "d:mixed",
    "d:single",
    "d:undecided",
    "d:stub",
    "d:premised",
    "d:bet-high",
    "d:high-nothing",
    "d:backfilled-premise",
    "d:late-premise",
    "d:on-superseded",
    "d:challenged",
    "d:refuted-later",
    "d:missing",
];

/// Records that land on every rung of every floor, so a backend that agrees on all of them is not
/// agreeing on emptiness. `e:sourced` and `e:unsourced` pre-date every decision; `e:after` and
/// `e:after-too` are recorded after the decisions they are linked to.
///
/// - `d:solid`: a question, three described options, sourced evidence at capture.
/// - `d:placeholders`: no question; rejected options carrying only text a capture surface
///   generated; unsourced evidence at capture.
/// - `d:bare`: nothing but a title and a rationale.
/// - `d:later-evidence`: no evidence at capture; `e:after` linked afterwards.
/// - `d:backfilled`: no evidence at capture; `e:sourced`, recorded before it, linked afterwards.
/// - `d:mixed`: unsourced evidence at capture; sourced `e:after-too` linked afterwards.
/// - `d:single`: one option, the chosen one. `d:undecided`: two described options, none chosen.
/// - `d:stub`: named by a decision request and never proposed, so its node has no rationale.
/// - `d:premised`: two options and a medium confidence; follows from `d:solid` at capture.
/// - `d:bet-high`: high confidence over a declared bet (`h:bet`) alone.
/// - `d:high-nothing`: high confidence, nothing declared.
/// - `d:backfilled-premise`: follows from `d:solid`, attributed afterwards; `d:solid` pre-dates it.
/// - `d:late-premise`: follows from `d:after-premise`, which was recorded after it.
/// - `d:on-superseded`: follows from `d:old-premise` at capture; `d:new-premise` supersedes that
///   afterwards.
/// - `d:challenged`: rests on `h:doubtful`, which `e:counter` refuted before the decision.
///   `d:refuted-later` rests on `h:later-doubt`, refuted only after it.
pub(crate) fn floor_scenario() -> Result<Scenario> {
    let s = Scenario::new();
    let crew = "agent:claude:crew";
    s.evidence(
        "e:sourced",
        "27 of 27 captures carry no premise",
        Some("mayor audit 2026-09-22"),
        "2026-01-01T00:00:01Z",
    )?;
    s.evidence(
        "e:unsourced",
        "Two users asked for it",
        None,
        "2026-01-01T00:00:02Z",
    )?;

    s.proposal(
        crew,
        "2026-01-02T00:00:00Z",
        record_payload(
            "d:solid",
            Some("Which store should hold the ledger?"),
            &[
                ("d:solid:pg", "Postgres", "One server for every tenant"),
                (
                    "d:solid:sqlite",
                    "SQLite",
                    "A file per tenant; rejected, no cross-tenant queries",
                ),
                (
                    "d:solid:kuzu",
                    "Kuzu",
                    "Embedded graph; rejected, its C++ build is too slow",
                ),
            ],
            Some("d:solid:pg"),
            &["e:sourced"],
        ),
    )?;
    s.proposal(
        crew,
        "2026-01-02T00:00:01Z",
        record_payload(
            "d:placeholders",
            None,
            &[
                (
                    "d:placeholders:a",
                    "Gateway",
                    "Route every call through one gateway",
                ),
                (
                    "d:placeholders:b",
                    "Direct",
                    "Option generated from MCP value 'Direct'",
                ),
                (
                    "d:placeholders:c",
                    "Queue",
                    "Option generated from CLI value 'Queue'",
                ),
            ],
            Some("d:placeholders:a"),
            &["e:unsourced"],
        ),
    )?;
    s.decision("d:bare", "Keep the default", crew, "2026-01-02T00:00:02Z")?;

    s.proposal(
        crew,
        "2026-01-02T00:00:03Z",
        record_payload("d:later-evidence", None, &[], None, &[]),
    )?;
    s.evidence(
        "e:after",
        "A benchmark run after the call was made",
        Some("bench run 2026-02-01"),
        "2026-02-01T00:00:00Z",
    )?;
    s.relation(
        "BASED_ON",
        "d:later-evidence",
        "e:after",
        "human:alex",
        None,
        "2026-02-01T00:00:01Z",
    )?;

    s.proposal(
        crew,
        "2026-01-02T00:00:04Z",
        record_payload("d:backfilled", None, &[], None, &[]),
    )?;
    s.relation(
        "BASED_ON",
        "d:backfilled",
        "e:sourced",
        "human:alex",
        None,
        "2026-02-01T00:00:02Z",
    )?;

    s.proposal(
        crew,
        "2026-01-02T00:00:05Z",
        record_payload("d:mixed", None, &[], None, &["e:unsourced"]),
    )?;
    s.evidence(
        "e:after-too",
        "A second benchmark run",
        Some("bench run 2026-02-02"),
        "2026-02-02T00:00:00Z",
    )?;
    s.relation(
        "BASED_ON",
        "d:mixed",
        "e:after-too",
        "human:alex",
        None,
        "2026-02-02T00:00:01Z",
    )?;

    s.proposal(
        crew,
        "2026-01-02T00:00:06Z",
        record_payload(
            "d:single",
            None,
            &[("d:single:a", "Only way", "The one option on the table")],
            Some("d:single:a"),
            &[],
        ),
    )?;
    s.proposal(
        crew,
        "2026-01-02T00:00:07Z",
        record_payload(
            "d:undecided",
            None,
            &[
                ("d:undecided:a", "Shard by tenant", "One shard per tenant"),
                ("d:undecided:b", "Shard by hash", "Hash the decision id"),
            ],
            None,
            &[],
        ),
    )?;
    s.request_naming("d:stub", "2026-01-02T00:00:08Z")?;
    add_grounded_decisions(&s, crew)?;
    Ok(s)
}

/// The decisions of `floor_scenario` that rest on a prior decision, an assumption or a bet, or
/// declare a confidence.
fn add_grounded_decisions(s: &Scenario, crew: &str) -> Result<()> {
    s.hypothesis(
        "h:bet",
        "Load stays flat",
        "bet",
        Some("2026-06-01T00:00:00Z"),
        "2026-01-03T00:00:00Z",
    )?;
    s.hypothesis(
        "h:doubtful",
        "Users tolerate a slow first load",
        "assumption",
        None,
        "2026-01-03T00:00:01Z",
    )?;
    s.evidence(
        "e:counter",
        "First load took nine seconds in the field test",
        Some("field test 2026-01-02"),
        "2026-01-03T00:00:02Z",
    )?;
    s.relation(
        "REFUTES",
        "e:counter",
        "h:doubtful",
        "agent:tester",
        None,
        "2026-01-03T00:00:03Z",
    )?;

    let mut premised = record_payload(
        "d:premised",
        None,
        &[
            (
                "d:premised:a",
                "Reuse the store",
                "Keep the ledger store we already chose",
            ),
            (
                "d:premised:b",
                "Switch again",
                "Move to a new store; rejected, the churn is not worth it",
            ),
        ],
        Some("d:premised:a"),
        &[],
    );
    premised["expressed_confidence"] = json!("medium");
    let proposal = s.proposal(crew, "2026-03-01T00:00:00Z", premised)?;
    s.relation(
        "FOLLOWS_FROM",
        "d:premised",
        "d:solid",
        crew,
        Some(proposal),
        "2026-03-01T00:00:01Z",
    )?;

    s.decision_with(
        "d:bet-high",
        "Ship on the bet",
        crew,
        "2026-03-02T00:00:00Z",
        false,
        &[],
        &["h:bet"],
        Some("high"),
    )?;
    s.decision_with(
        "d:high-nothing",
        "Trust the vendor",
        crew,
        "2026-03-02T00:00:01Z",
        false,
        &[],
        &[],
        Some("high"),
    )?;

    s.decision(
        "d:backfilled-premise",
        "Keep the store",
        crew,
        "2026-03-02T00:00:02Z",
    )?;
    s.relation(
        "FOLLOWS_FROM",
        "d:backfilled-premise",
        "d:solid",
        "human:alex",
        None,
        "2026-04-01T00:00:00Z",
    )?;

    s.decision(
        "d:late-premise",
        "Keep the queue",
        crew,
        "2026-03-02T00:00:03Z",
    )?;
    s.decision(
        "d:after-premise",
        "Queue sizing rule",
        crew,
        "2026-03-03T00:00:00Z",
    )?;
    s.relation(
        "FOLLOWS_FROM",
        "d:late-premise",
        "d:after-premise",
        "human:alex",
        None,
        "2026-04-01T00:00:01Z",
    )?;

    s.decision(
        "d:old-premise",
        "Bill per call",
        crew,
        "2026-03-02T00:00:04Z",
    )?;
    let proposal = s.decision(
        "d:on-superseded",
        "Cache billing reads",
        crew,
        "2026-03-02T00:00:05Z",
    )?;
    s.relation(
        "FOLLOWS_FROM",
        "d:on-superseded",
        "d:old-premise",
        crew,
        Some(proposal),
        "2026-03-02T00:00:06Z",
    )?;
    s.decision(
        "d:new-premise",
        "Bill per seat",
        crew,
        "2026-03-05T00:00:00Z",
    )?;
    s.supersede(
        "d:old-premise",
        "d:new-premise",
        crew,
        "2026-03-05T00:00:01Z",
    )?;

    s.decision_with(
        "d:challenged",
        "Ship the slow first load",
        crew,
        "2026-03-06T00:00:00Z",
        false,
        &[],
        &["h:doubtful"],
        None,
    )?;
    s.hypothesis(
        "h:later-doubt",
        "Users will not notice",
        "assumption",
        None,
        "2026-03-06T00:00:01Z",
    )?;
    s.decision_with(
        "d:refuted-later",
        "Ship the unnoticed change",
        crew,
        "2026-03-06T00:00:02Z",
        false,
        &[],
        &["h:later-doubt"],
        None,
    )?;
    s.evidence(
        "e:counter-later",
        "Users noticed within a day",
        Some("support inbox 2026-03-08"),
        "2026-03-08T00:00:00Z",
    )?;
    s.relation(
        "REFUTES",
        "e:counter-later",
        "h:later-doubt",
        "agent:tester",
        None,
        "2026-03-08T00:00:01Z",
    )
}

/// The clock `attention_scenario` is read at; its evidence window (90 days) reaches back to
/// 2026-06-28T00:00:00Z.
pub(crate) const ATTENTION_NOW: &str = "2026-09-26T00:00:00Z";

/// Decisions that need a look, and their look-alikes that do not, for the attention findings.
/// Read at `ATTENTION_NOW`. Every decision is proposed by `agent:claude:crew`.
///
/// Bets (`h:bet-*`, check dates in `h:*` names): `d:bet-overdue` (through its chosen option) and
/// `d:bet-overdue-direct` (directly) share an overdue bet; `d:bet-future`, `d:bet-undated` and
/// `d:bet-held` (supported) are not overdue; `d:bet-failed` rests on a refuted bet;
/// `d:bet-dead` (superseded) and `d:bet-refused` (rejected) rest on the overdue bet too but no
/// longer stand. `d:assumption-refuted` rests on an assumption refuted twice (the earlier
/// refutation has the later id); `d:assumption-open` on one nothing refutes.
///
/// Premises: `d:p-old` was superseded by `d:p-new`; `d:p-double` by `d:p-double-a` (2026-08-10)
/// and `d:p-double-b` (2026-08-05, the earlier); `d:p-rejected` was rejected; `d:p-contested` was
/// accepted and rejected; `d:p-standing` stands. `d:f-superseded`, `d:f-concurrent`,
/// `d:f-rejected` and `d:f-both` (superseded and rejected premise) follow from them and are
/// flagged; `d:f-contested` and `d:f-standing` are not; `d:f-dead` (superseded) and
/// `d:f-refused` (rejected) follow from `d:p-old` but no longer stand. `d:cycle-a` and
/// `d:cycle-b` follow from each other and `d:cycle-a` also from `d:p-old`; `d:cycle-self`
/// follows from itself.
///
/// Evidence (recorded dates in `e:*`): `d:ev-stale` cites only `e:old`; `d:ev-stale-two` cites
/// `e:very-old` and `e:old`; `d:ev-fresh` a recent item; `d:ev-mixed` an old and a recent one;
/// `d:ev-rechecked-later` an old one at capture and a recent one linked afterwards;
/// `d:ev-just-over` an item one second past the window and `d:ev-boundary` one exactly at it;
/// `d:ev-rejected` cites `e:old` but was rejected; `d:ev-none` cites nothing.
pub(crate) fn attention_scenario() -> Result<Scenario> {
    let s = Scenario::new();
    let crew = "agent:claude:crew";
    let alex = "human:alex";

    // Bets and assumptions.
    s.hypothesis(
        "h:bet-overdue",
        "Load stays flat",
        "bet",
        Some("2026-09-01T00:00:00Z"),
        "2026-01-01T00:00:00Z",
    )?;
    s.hypothesis(
        "h:bet-future",
        "Vendor ships in Q4",
        "bet",
        Some("2026-12-01T00:00:00Z"),
        "2026-01-01T00:00:01Z",
    )?;
    s.hypothesis(
        "h:bet-undated",
        "Nobody else builds it",
        "bet",
        None,
        "2026-01-01T00:00:02Z",
    )?;
    s.hypothesis(
        "h:bet-held",
        "The cache holds",
        "bet",
        Some("2026-08-01T00:00:00Z"),
        "2026-01-01T00:00:03Z",
    )?;
    s.hypothesis(
        "h:bet-failed",
        "Latency stays low",
        "bet",
        Some("2026-08-01T00:00:00Z"),
        "2026-01-01T00:00:04Z",
    )?;
    s.hypothesis(
        "h:assumption-refuted",
        "Users log in daily",
        "assumption",
        None,
        "2026-01-01T00:00:05Z",
    )?;
    s.hypothesis(
        "h:assumption-open",
        "The API is stable",
        "assumption",
        None,
        "2026-01-01T00:00:06Z",
    )?;

    // Evidence: what checks the bets, and what the decisions cite.
    s.evidence(
        "e:support",
        "The cache held under load",
        Some("load test"),
        "2026-08-20T00:00:00Z",
    )?;
    s.evidence(
        "e:refute-bet",
        "p95 doubled",
        Some("dashboard"),
        "2026-08-15T00:00:00Z",
    )?;
    s.evidence(
        "e:refute-a2",
        "Half of users log in weekly",
        Some("analytics"),
        "2026-06-01T00:00:00Z",
    )?;
    s.evidence(
        "e:refute-a1",
        "Most users log in monthly",
        Some("analytics"),
        "2026-07-01T00:00:00Z",
    )?;
    s.evidence(
        "e:very-old",
        "Benchmark from January",
        Some("bench run"),
        "2026-01-01T00:00:00Z",
    )?;
    s.evidence(
        "e:old",
        "Benchmark from March",
        Some("bench run"),
        "2026-03-01T00:00:00Z",
    )?;
    s.evidence(
        "e:just-over",
        "Just outside the window",
        None,
        "2026-06-27T23:59:59Z",
    )?;
    s.evidence(
        "e:boundary",
        "Exactly at the window",
        None,
        "2026-06-28T00:00:00Z",
    )?;
    s.evidence(
        "e:fresh",
        "Measured last week",
        Some("dashboard"),
        "2026-09-10T00:00:00Z",
    )?;
    s.evidence(
        "e:newer",
        "Measured this month",
        Some("dashboard"),
        "2026-09-15T00:00:00Z",
    )?;
    s.relation(
        "SUPPORTS",
        "e:support",
        "h:bet-held",
        alex,
        None,
        "2026-08-20T00:00:01Z",
    )?;
    s.relation(
        "REFUTES",
        "e:refute-bet",
        "h:bet-failed",
        alex,
        None,
        "2026-08-15T00:00:01Z",
    )?;
    s.relation(
        "REFUTES",
        "e:refute-a2",
        "h:assumption-refuted",
        alex,
        None,
        "2026-06-01T00:00:01Z",
    )?;
    s.relation(
        "REFUTES",
        "e:refute-a1",
        "h:assumption-refuted",
        alex,
        None,
        "2026-07-01T00:00:01Z",
    )?;

    // Decisions resting on bets and assumptions. A chosen option carries them at capture; without
    // one they hang off the decision.
    s.decision_with(
        "d:bet-overdue",
        "Size the fleet for flat load",
        crew,
        "2026-02-01T00:00:00Z",
        true,
        &[],
        &["h:bet-overdue"],
        None,
    )?;
    s.decision_with(
        "d:bet-overdue-direct",
        "Skip autoscaling",
        crew,
        "2026-02-01T00:00:01Z",
        false,
        &[],
        &["h:bet-overdue"],
        None,
    )?;
    s.decision_with(
        "d:bet-future",
        "Wait for the vendor",
        crew,
        "2026-02-01T00:00:02Z",
        true,
        &[],
        &["h:bet-future"],
        None,
    )?;
    s.decision_with(
        "d:bet-undated",
        "Build it ourselves",
        crew,
        "2026-02-01T00:00:03Z",
        true,
        &[],
        &["h:bet-undated"],
        None,
    )?;
    s.decision_with(
        "d:bet-held",
        "Cache the catalogue",
        crew,
        "2026-02-01T00:00:04Z",
        true,
        &[],
        &["h:bet-held"],
        None,
    )?;
    s.decision_with(
        "d:bet-failed",
        "Serve from one region",
        crew,
        "2026-02-01T00:00:05Z",
        true,
        &[],
        &["h:bet-failed"],
        None,
    )?;
    s.decision_with(
        "d:assumption-refuted",
        "Poll once a day",
        crew,
        "2026-02-01T00:00:06Z",
        true,
        &[],
        &["h:assumption-refuted"],
        None,
    )?;
    s.decision_with(
        "d:assumption-open",
        "Pin the API version",
        crew,
        "2026-02-01T00:00:07Z",
        true,
        &[],
        &["h:assumption-open"],
        None,
    )?;
    s.decision_with(
        "d:bet-dead",
        "Old fleet sizing",
        crew,
        "2026-02-01T00:00:08Z",
        true,
        &[],
        &["h:bet-overdue"],
        None,
    )?;
    s.decision(
        "d:bet-replacement",
        "New fleet sizing",
        crew,
        "2026-03-01T00:00:00Z",
    )?;
    s.supersede(
        "d:bet-dead",
        "d:bet-replacement",
        crew,
        "2026-03-01T00:00:01Z",
    )?;
    s.decision_with(
        "d:bet-refused",
        "Fleet sizing nobody wanted",
        crew,
        "2026-02-01T00:00:09Z",
        true,
        &[],
        &["h:bet-overdue"],
        None,
    )?;
    s.reject("d:bet-refused", alex, "2026-03-02T00:00:00Z")?;

    // Premises and the decisions that follow from them.
    s.decision("d:p-old", "Use the old queue", crew, "2026-02-01T00:00:10Z")?;
    s.decision("d:p-new", "Use the new queue", crew, "2026-08-01T00:00:00Z")?;
    s.supersede("d:p-old", "d:p-new", crew, "2026-08-01T00:00:01Z")?;
    s.decision("d:p-double", "Use one region", crew, "2026-02-01T00:00:11Z")?;
    s.decision(
        "d:p-double-a",
        "Use two regions",
        crew,
        "2026-08-10T00:00:00Z",
    )?;
    s.decision(
        "d:p-double-b",
        "Use three regions",
        crew,
        "2026-08-05T00:00:00Z",
    )?;
    s.supersede("d:p-double", "d:p-double-a", crew, "2026-08-10T00:00:01Z")?;
    s.supersede("d:p-double", "d:p-double-b", crew, "2026-08-10T00:00:02Z")?;
    s.decision(
        "d:p-rejected",
        "Adopt the vendor SDK",
        crew,
        "2026-02-01T00:00:12Z",
    )?;
    s.reject("d:p-rejected", alex, "2026-03-03T00:00:00Z")?;
    s.decision(
        "d:p-contested",
        "Adopt the open SDK",
        crew,
        "2026-02-01T00:00:13Z",
    )?;
    s.accept("d:p-contested", "human:alice", "2026-03-04T00:00:00Z")?;
    s.reject("d:p-contested", "human:bob", "2026-03-04T00:00:01Z")?;
    s.decision(
        "d:p-standing",
        "Adopt the platform SDK",
        crew,
        "2026-02-01T00:00:14Z",
    )?;
    s.accept("d:p-standing", alex, "2026-03-05T00:00:00Z")?;

    let follow =
        |decision_id: &str, title: &str, timestamp: &str, premises: &[&str]| -> Result<()> {
            let proposal = s.decision(decision_id, title, crew, timestamp)?;
            for premise in premises {
                s.relation(
                    "FOLLOWS_FROM",
                    decision_id,
                    premise,
                    crew,
                    Some(proposal),
                    timestamp,
                )?;
            }
            Ok(())
        };
    follow(
        "d:f-superseded",
        "Batch jobs on the old queue",
        "2026-04-01T00:00:00Z",
        &["d:p-old"],
    )?;
    follow(
        "d:f-concurrent",
        "Pin the region count",
        "2026-04-01T00:00:01Z",
        &["d:p-double"],
    )?;
    follow(
        "d:f-rejected",
        "Wrap the vendor SDK",
        "2026-04-01T00:00:02Z",
        &["d:p-rejected"],
    )?;
    follow(
        "d:f-both",
        "Vendor SDK on the old queue",
        "2026-04-01T00:00:03Z",
        &["d:p-old", "d:p-rejected"],
    )?;
    follow(
        "d:f-contested",
        "Document the open SDK",
        "2026-04-01T00:00:04Z",
        &["d:p-contested"],
    )?;
    follow(
        "d:f-standing",
        "Document the platform SDK",
        "2026-04-01T00:00:05Z",
        &["d:p-standing"],
    )?;
    follow(
        "d:f-dead",
        "Old batch jobs",
        "2026-04-01T00:00:06Z",
        &["d:p-old"],
    )?;
    s.decision(
        "d:f-replacement",
        "New batch jobs",
        crew,
        "2026-05-01T00:00:00Z",
    )?;
    s.supersede("d:f-dead", "d:f-replacement", crew, "2026-05-01T00:00:01Z")?;
    follow(
        "d:f-refused",
        "Refused batch jobs",
        "2026-04-01T00:00:07Z",
        &["d:p-old"],
    )?;
    s.reject("d:f-refused", alex, "2026-05-02T00:00:00Z")?;
    // A cycle needs both decisions to exist before the links that close it.
    s.decision("d:cycle-a", "Cycle A", crew, "2026-04-01T00:00:08Z")?;
    s.decision("d:cycle-b", "Cycle B", crew, "2026-04-01T00:00:09Z")?;
    s.decision(
        "d:cycle-self",
        "Cycle on itself",
        crew,
        "2026-04-01T00:00:10Z",
    )?;
    for (from, to) in [
        ("d:cycle-a", "d:cycle-b"),
        ("d:cycle-a", "d:p-old"),
        ("d:cycle-b", "d:cycle-a"),
        ("d:cycle-self", "d:cycle-self"),
    ] {
        s.relation("FOLLOWS_FROM", from, to, crew, None, "2026-04-01T00:00:11Z")?;
    }

    // Decisions citing evidence.
    s.decision_with(
        "d:ev-stale",
        "Trust the March benchmark",
        crew,
        "2026-04-02T00:00:00Z",
        false,
        &["e:old"],
        &[],
        None,
    )?;
    s.decision_with(
        "d:ev-stale-two",
        "Trust both benchmarks",
        crew,
        "2026-04-02T00:00:01Z",
        false,
        &["e:very-old", "e:old"],
        &[],
        None,
    )?;
    s.decision_with(
        "d:ev-fresh",
        "Trust the dashboard",
        crew,
        "2026-09-11T00:00:00Z",
        false,
        &["e:fresh"],
        &[],
        None,
    )?;
    s.decision_with(
        "d:ev-mixed",
        "Trust the old and the new",
        crew,
        "2026-09-16T00:00:00Z",
        false,
        &["e:very-old", "e:newer"],
        &[],
        None,
    )?;
    s.decision_with(
        "d:ev-rechecked-later",
        "Trust the January benchmark",
        crew,
        "2026-04-02T00:00:02Z",
        false,
        &["e:very-old"],
        &[],
        None,
    )?;
    s.relation(
        "BASED_ON",
        "d:ev-rechecked-later",
        "e:newer",
        alex,
        None,
        "2026-09-15T00:00:01Z",
    )?;
    s.decision_with(
        "d:ev-just-over",
        "Trust the June note",
        crew,
        "2026-06-29T00:00:00Z",
        false,
        &["e:just-over"],
        &[],
        None,
    )?;
    s.decision_with(
        "d:ev-boundary",
        "Trust the other June note",
        crew,
        "2026-06-29T00:00:01Z",
        false,
        &["e:boundary"],
        &[],
        None,
    )?;
    s.decision_with(
        "d:ev-rejected",
        "Trust a benchmark nobody wanted",
        crew,
        "2026-04-02T00:00:03Z",
        false,
        &["e:old"],
        &[],
        None,
    )?;
    s.reject("d:ev-rejected", alex, "2026-05-03T00:00:00Z")?;
    s.decision(
        "d:ev-none",
        "Decided without evidence",
        crew,
        "2026-04-02T00:00:04Z",
    )?;
    Ok(s)
}

/// The projects and decisions behind the project-first "what should I know" answer
/// (hivemind-s15q.6). Links, `part_of` up and `depends_on` across:
///
/// ```text
/// Billing --part_of--> Platform --part_of--> City
///    |                    `--depends_on--> Infra
///    `--depends_on--> Auth --depends_on--> Crypto        Marketing (linked to nothing)
/// ```
///
/// One decision on the topic `pricing` sits in each project, `auth_old` is superseded by
/// `auth_new` (which inherits Auth), and `personal` was recorded with no project at all. Asked
/// from Billing, the answer is Billing, Platform, then Auth's two; it leaves out City (one level
/// too high), Crypto and Infra (one link too far), Marketing and the personal decision.
pub(crate) struct ProjectFirstFixture {
    pub(crate) ledger: InMemoryEventLedger,
    pub(crate) billing: String,
    pub(crate) platform: String,
    pub(crate) city: String,
    pub(crate) auth_old: String,
    pub(crate) auth_new: String,
    pub(crate) crypto: String,
    pub(crate) infra: String,
    pub(crate) marketing: String,
    pub(crate) personal: String,
}

fn propose_pricing(
    commands: &Commands<'_, InMemoryEventLedger>,
    title: &str,
    project: Option<&str>,
) -> Result<String> {
    let actor = "human:alex";
    let option_id = commands.record_option(actor, "Per seat", "Charge each seat")?;
    commands.propose_decision(DecisionProposalInput {
        grounding: Grounding::NotAsked,
        expressed_confidence: None,
        actor_id: actor,
        title,
        rationale: "Scoping a question by project keeps the answer to what applies here",
        topic_keys: &["pricing".to_owned()],
        option_ids: std::slice::from_ref(&option_id),
        option_labels: &["Per seat".to_owned()],
        chosen_option_id: Some(option_id.as_str()),
        decided_by: None,
        still_proposed: false,
        hypothesis_ids: &[],
        evidence_ids: &[],
        quote: None,
        question: None,
        delegated_by: None,
        project: project.map(DeterminedProject::stated),
    })
}

pub(crate) fn project_first_fixture() -> Result<ProjectFirstFixture> {
    let ledger = InMemoryEventLedger::new();
    let commands = Commands::new(&ledger);
    let alex = "human:alex";
    for (handle, display_name) in [
        ("city", Some("City")),
        ("platform", Some("Platform")),
        ("billing", Some("Billing")),
        ("auth", Some("Auth")),
        ("crypto", None),
        ("infra", None),
        ("marketing", Some("Marketing")),
    ] {
        commands.register_project(alex, handle, display_name, None)?;
    }
    commands.link_project(alex, "platform", "city", ProjectLinkKind::PartOf)?;
    commands.link_project(alex, "billing", "platform", ProjectLinkKind::PartOf)?;
    commands.link_project(alex, "billing", "auth", ProjectLinkKind::DependsOn)?;
    commands.link_project(alex, "auth", "crypto", ProjectLinkKind::DependsOn)?;
    commands.link_project(alex, "platform", "infra", ProjectLinkKind::DependsOn)?;

    let billing = propose_pricing(&commands, "Price per seat", Some("billing"))?;
    let platform = propose_pricing(&commands, "Quote every price in euros", Some("platform"))?;
    let city = propose_pricing(&commands, "No free tiers anywhere", Some("city"))?;
    let auth_old = propose_pricing(&commands, "Bill token issuance per call", Some("auth"))?;
    let crypto = propose_pricing(&commands, "Rotate signing keys monthly", Some("crypto"))?;
    let infra = propose_pricing(&commands, "Run on shared hosts", Some("infra"))?;
    let marketing = propose_pricing(&commands, "Launch with a promo price", Some("marketing"))?;
    let personal = propose_pricing(&commands, "Keep a personal scratch price", None)?;
    let auth_new = commands
        .supersede(SupersedeInput {
            actor_id: alex,
            old_decision_id: &auth_old,
            new_title: "Bill token issuance per seat",
            new_rationale: "Per-call billing surprised customers; seats are predictable",
            topic_keys: &["pricing".to_owned()],
            option_labels: &["Per seat".to_owned()],
            chosen_option_label: Some("Per seat"),
            hypothesis_ids: &[],
            evidence_ids: &[],
            project: None,
            grounding: None,
            expressed_confidence: None,
        })?
        .new_decision_id;

    Ok(ProjectFirstFixture {
        ledger,
        billing,
        platform,
        city,
        auth_old,
        auth_new,
        crypto,
        infra,
        marketing,
        personal,
    })
}
