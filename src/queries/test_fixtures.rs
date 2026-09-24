// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use std::cell::Cell;

use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::events::{Event, EventId, EventSource, EventType};
use crate::ledger::{EventLedger, InMemoryEventLedger};
use crate::projector::{memory::MemoryGraph, rebuild_graph};
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
            source_ref: None,
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
