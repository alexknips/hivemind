use std::collections::HashSet;
use std::sync::{Mutex, MutexGuard};

use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

use crate::error::CommandError;
use crate::events::{Event, EventId, EventProvenance, EventType, RelationKind};
use crate::ledger::EventLedger;
use crate::Result;

pub type DecisionId = String;
pub type EvidenceId = String;
pub type HypothesisId = String;
pub type OptionId = String;

pub const MAX_TOPIC_KEY_LEN: usize = 64;

pub struct Commands<'a, L: EventLedger> {
    ledger: &'a L,
    provenance: EventProvenance,
    state: Mutex<CommandState>,
}

#[derive(Default)]
struct CommandState {
    option_ids: HashSet<OptionId>,
}

impl<'a, L: EventLedger> Commands<'a, L> {
    pub fn new(ledger: &'a L) -> Self {
        Self::new_with_provenance(ledger, EventProvenance::cli())
    }

    pub fn new_with_provenance(ledger: &'a L, provenance: EventProvenance) -> Self {
        Self {
            ledger,
            provenance,
            state: Mutex::new(CommandState::default()),
        }
    }

    pub fn record_evidence(&self, actor_id: &str, content: &str) -> Result<EvidenceId> {
        require_non_empty("actor_id", actor_id)?;
        require_non_empty("content", content)?;

        let evidence_id = generate_entity_id("evidence");
        let event = self.event(
            EventType::EvidenceRecorded,
            actor_id,
            json!({
                "evidence_id": evidence_id,
                "content": content,
                "source": null
            }),
            None,
        );

        self.ledger.append(event)?;
        Ok(evidence_id)
    }

    pub fn record_hypothesis(&self, actor_id: &str, statement: &str) -> Result<HypothesisId> {
        require_non_empty("actor_id", actor_id)?;
        require_non_empty("statement", statement)?;

        let hypothesis_id = generate_entity_id("hypothesis");
        let event = self.event(
            EventType::HypothesisRecorded,
            actor_id,
            json!({
                "hypothesis_id": hypothesis_id,
                "statement": statement
            }),
            None,
        );

        self.ledger.append(event)?;
        Ok(hypothesis_id)
    }

    pub fn record_option(
        &self,
        actor_id: &str,
        label: &str,
        description: &str,
    ) -> Result<OptionId> {
        require_non_empty("actor_id", actor_id)?;
        require_non_empty("label", label)?;
        require_non_empty("description", description)?;

        let option_id = generate_entity_id("option");
        let mut state = self.lock_state()?;
        state.option_ids.insert(option_id.clone());
        Ok(option_id)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn propose_decision(
        &self,
        actor_id: &str,
        title: &str,
        rationale: &str,
        topic_keys: &[String],
        option_ids: &[String],
        chosen_option_id: Option<&str>,
        hypothesis_ids: &[String],
        evidence_ids: &[String],
    ) -> Result<DecisionId> {
        require_non_empty("actor_id", actor_id)?;
        require_non_empty("title", title)?;
        require_non_empty("rationale", rationale)?;

        if option_ids.is_empty() {
            return Err(CommandError::Validation("option_ids must not be empty".to_owned()).into());
        }

        let normalized_topic_keys: Vec<String> = topic_keys
            .iter()
            .map(|topic| normalize_topic_key(topic))
            .filter(|topic| !topic.is_empty())
            .collect();

        if normalized_topic_keys.is_empty() {
            return Err(CommandError::Validation(
                "topic_keys must contain at least one non-empty normalized key".to_owned(),
            )
            .into());
        }

        {
            let state = self.lock_state()?;
            for option_id in option_ids {
                if !state.option_ids.contains(option_id) {
                    return Err(CommandError::Invariant(format!(
                        "option does not exist: {option_id}"
                    ))
                    .into());
                }
            }
        }

        if let Some(chosen_option_id) = chosen_option_id {
            let chosen_option_is_candidate = option_ids.iter().any(|option_id| {
                // ubs:ignore: option IDs are public decision graph IDs, not secrets.
                same_identifier(option_id, chosen_option_id)
            });
            if !chosen_option_is_candidate {
                return Err(CommandError::Validation(
                    "chosen_option_id must be one of option_ids".to_owned(),
                )
                .into());
            }
        }

        for hypothesis_id in hypothesis_ids {
            if !self.hypothesis_exists(hypothesis_id)? {
                return Err(CommandError::Invariant(format!(
                    "hypothesis does not exist: {hypothesis_id}"
                ))
                .into());
            }
        }

        for evidence_id in evidence_ids {
            if !self.evidence_exists(evidence_id)? {
                return Err(CommandError::Invariant(format!(
                    "evidence does not exist: {evidence_id}"
                ))
                .into());
            }
        }

        let decision_id = generate_entity_id("decision");

        let root_event = self.event(
            EventType::DecisionProposed,
            actor_id,
            json!({
                "decision_id": decision_id,
                "title": title,
                "rationale": rationale,
                "topic_keys": normalized_topic_keys,
                "option_ids": option_ids,
                "chosen_option_id": chosen_option_id,
                "hypothesis_ids": hypothesis_ids,
                "evidence_ids": evidence_ids,
            }),
            None,
        );

        let root_event_id = self.ledger.append(root_event)?;

        for option_id in option_ids {
            self.append_relation_event(
                actor_id,
                root_event_id,
                RelationKind::HasOption,
                &decision_id,
                option_id,
            )?;
        }

        if let Some(chosen_option_id) = chosen_option_id {
            self.append_relation_event(
                actor_id,
                root_event_id,
                RelationKind::Chose,
                &decision_id,
                chosen_option_id,
            )?;
        }

        for hypothesis_id in hypothesis_ids {
            self.append_relation_event(
                actor_id,
                root_event_id,
                RelationKind::Assumes,
                &decision_id,
                hypothesis_id,
            )?;
        }

        for evidence_id in evidence_ids {
            self.append_relation_event(
                actor_id,
                root_event_id,
                RelationKind::BasedOn,
                &decision_id,
                evidence_id,
            )?;
        }

        Ok(decision_id)
    }

    pub fn accept_decision(&self, decision_id: &str, actor_id: &str) -> Result<EventId> {
        require_non_empty("actor_id", actor_id)?;
        require_non_empty("decision_id", decision_id)?;

        if !self.decision_exists(decision_id)? {
            return Err(
                CommandError::Invariant(format!("decision does not exist: {decision_id}")).into(),
            );
        }

        if self.actor_has_decision_event(decision_id, actor_id, EventType::DecisionRejected)? {
            return Err(CommandError::Invariant(format!(
                "actor {actor_id} cannot accept and reject decision {decision_id}"
            ))
            .into());
        }

        let event = self.event(
            EventType::DecisionAccepted,
            actor_id,
            json!({ "decision_id": decision_id }),
            None,
        );

        self.ledger.append(event)
    }

    pub fn reject_decision(&self, decision_id: &str, actor_id: &str) -> Result<EventId> {
        require_non_empty("actor_id", actor_id)?;
        require_non_empty("decision_id", decision_id)?;

        if !self.decision_exists(decision_id)? {
            return Err(
                CommandError::Invariant(format!("decision does not exist: {decision_id}")).into(),
            );
        }

        if self.actor_has_decision_event(decision_id, actor_id, EventType::DecisionAccepted)? {
            return Err(CommandError::Invariant(format!(
                "actor {actor_id} cannot accept and reject decision {decision_id}"
            ))
            .into());
        }

        let event = self.event(
            EventType::DecisionRejected,
            actor_id,
            json!({ "decision_id": decision_id }),
            None,
        );

        self.ledger.append(event)
    }

    pub fn supersede_decision(
        &self,
        old_decision_id: &str,
        new_decision_id: &str,
        actor_id: &str,
    ) -> Result<EventId> {
        require_non_empty("actor_id", actor_id)?;
        require_non_empty("old_decision_id", old_decision_id)?;
        require_non_empty("new_decision_id", new_decision_id)?;

        if same_identifier(old_decision_id, new_decision_id) {
            return Err(CommandError::Validation(
                "old_decision_id and new_decision_id must be different".to_owned(),
            )
            .into());
        }

        if !self.decision_exists(old_decision_id)? {
            return Err(CommandError::Invariant(format!(
                "decision does not exist: {old_decision_id}"
            ))
            .into());
        }

        if !self.decision_exists(new_decision_id)? {
            return Err(CommandError::Invariant(format!(
                "decision does not exist: {new_decision_id}"
            ))
            .into());
        }

        let event = self.event(
            EventType::DecisionSuperseded,
            actor_id,
            json!({
                "old_decision_id": old_decision_id,
                "new_decision_id": new_decision_id,
            }),
            None,
        );

        self.ledger.append(event)
    }

    pub fn attach_evidence(
        &self,
        decision_id: &str,
        evidence_id: &str,
        actor_id: &str,
    ) -> Result<EventId> {
        require_non_empty("actor_id", actor_id)?;
        require_non_empty("decision_id", decision_id)?;
        require_non_empty("evidence_id", evidence_id)?;

        if !self.decision_exists(decision_id)? {
            return Err(
                CommandError::Invariant(format!("decision does not exist: {decision_id}")).into(),
            );
        }
        if !self.evidence_exists(evidence_id)? {
            return Err(
                CommandError::Invariant(format!("evidence does not exist: {evidence_id}")).into(),
            );
        }

        self.append_relation_event(actor_id, 0, RelationKind::BasedOn, decision_id, evidence_id)
    }

    pub fn relate_evidence_to_hypothesis(
        &self,
        evidence_id: &str,
        hypothesis_id: &str,
        relation_kind: RelationKind,
        actor_id: &str,
    ) -> Result<EventId> {
        require_non_empty("actor_id", actor_id)?;
        require_non_empty("evidence_id", evidence_id)?;
        require_non_empty("hypothesis_id", hypothesis_id)?;

        if !matches!(
            relation_kind,
            RelationKind::Supports | RelationKind::Refutes
        ) {
            return Err(CommandError::Validation(
                "relation kind must be supports or refutes".to_owned(),
            )
            .into());
        }

        if !self.evidence_exists(evidence_id)? {
            return Err(
                CommandError::Invariant(format!("evidence does not exist: {evidence_id}")).into(),
            );
        }
        if !self.hypothesis_exists(hypothesis_id)? {
            return Err(CommandError::Invariant(format!(
                "hypothesis does not exist: {hypothesis_id}"
            ))
            .into());
        }

        if let Some(existing_event_id) =
            self.find_relation_event_id(relation_kind, evidence_id, hypothesis_id)?
        {
            return Ok(existing_event_id);
        }

        self.append_relation_event(actor_id, 0, relation_kind, evidence_id, hypothesis_id)
    }

    fn append_relation_event(
        &self,
        actor_id: &str,
        root_event_id: EventId,
        relation: RelationKind,
        from_id: &str,
        to_id: &str,
    ) -> Result<EventId> {
        let event = self.event(
            EventType::RelationAdded,
            actor_id,
            json!({
                "relation": relation,
                "from_id": from_id,
                "to_id": to_id,
            }),
            if root_event_id == 0 {
                None
            } else {
                Some(root_event_id)
            },
        );

        self.ledger.append(event)
    }

    fn event(
        &self,
        event_type: EventType,
        actor_id: &str,
        payload: serde_json::Value,
        causation_event_id: Option<EventId>,
    ) -> Event {
        Event {
            event_id: None,
            event_uuid: Uuid::new_v4(),
            correlation_id: None,
            causation_event_id,
            event_type,
            actor_id: actor_id.to_owned(),
            source: self.provenance.source,
            source_ref: self.provenance.source_ref.clone(),
            payload,
            ts: Some(Utc::now()),
        }
    }

    fn lock_state(&self) -> Result<MutexGuard<'_, CommandState>> {
        self.state.lock().map_err(|error| {
            CommandError::Invariant(format!("commands state lock poisoned: {error}")).into()
        })
    }

    fn evidence_exists(&self, evidence_id: &str) -> Result<bool> {
        self.scan_events(|event| {
            // ubs:ignore: evidence IDs are public ledger IDs, not timing-sensitive secrets.
            let has_matching_id = payload_value_matches(event, "evidence_id", evidence_id);
            event.event_type == EventType::EvidenceRecorded && has_matching_id
        })
    }

    fn hypothesis_exists(&self, hypothesis_id: &str) -> Result<bool> {
        self.scan_events(|event| {
            let has_matching_id =
                // ubs:ignore: hypothesis IDs are public ledger IDs, not timing-sensitive secrets.
                payload_value_matches(event, "hypothesis_id", hypothesis_id);
            event.event_type == EventType::HypothesisRecorded && has_matching_id
        })
    }

    fn decision_exists(&self, decision_id: &str) -> Result<bool> {
        self.scan_events(|event| {
            // ubs:ignore: decision IDs are public ledger IDs, not timing-sensitive secrets.
            let has_matching_id = payload_value_matches(event, "decision_id", decision_id);
            event.event_type == EventType::DecisionProposed && has_matching_id
        })
    }

    fn actor_has_decision_event(
        &self,
        decision_id: &str,
        actor_id: &str,
        decision_event_type: EventType,
    ) -> Result<bool> {
        self.scan_events(|event| {
            // ubs:ignore: actor IDs are public ledger IDs, not timing-sensitive secrets.
            let has_matching_actor = same_identifier(event.actor_id.as_str(), actor_id);
            // ubs:ignore: decision IDs are public ledger IDs, not timing-sensitive secrets.
            let has_matching_id = payload_value_matches(event, "decision_id", decision_id);
            event.event_type == decision_event_type && has_matching_actor && has_matching_id
        })
    }

    fn find_relation_event_id(
        &self,
        relation_kind: RelationKind,
        from_id: &str,
        to_id: &str,
    ) -> Result<Option<EventId>> {
        let mut offset = 0;
        const PAGE_SIZE: usize = 1024;
        let relation_name = relation_kind_name(relation_kind);

        loop {
            let events = self.ledger.read(offset, PAGE_SIZE)?;
            if events.is_empty() {
                return Ok(None);
            }

            for event in &events {
                if event.event_type != EventType::RelationAdded {
                    continue;
                }

                let same_relation = payload_value_matches(event, "relation", relation_name);
                let same_from = payload_value_matches(event, "from_id", from_id);
                let same_to = payload_value_matches(event, "to_id", to_id);
                if same_relation && same_from && same_to {
                    return Ok(event.event_id);
                }
            }

            if let Some(last_event_id) = events.last().and_then(|event| event.event_id) {
                offset = last_event_id;
            } else {
                return Ok(None);
            }
        }
    }

    fn scan_events(&self, predicate: impl Fn(&Event) -> bool) -> Result<bool> {
        let mut offset = 0;
        const PAGE_SIZE: usize = 1024;

        loop {
            let events = self.ledger.read(offset, PAGE_SIZE)?;
            if events.is_empty() {
                return Ok(false);
            }

            for event in &events {
                if predicate(event) {
                    return Ok(true);
                }
            }

            if let Some(last_event_id) = events.last().and_then(|event| event.event_id) {
                offset = last_event_id;
            } else {
                return Ok(false);
            }
        }
    }
}

pub fn normalize_topic_key(input: &str) -> String {
    let transliterated = deunicode::deunicode(input);
    let mut normalized = String::with_capacity(transliterated.len());
    let mut last_was_separator = false;

    for character in transliterated.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            normalized.push(character);
            last_was_separator = false;
            continue;
        }

        if normalized.is_empty() || last_was_separator {
            continue;
        }

        normalized.push('-');
        last_was_separator = true;
    }

    while normalized.ends_with('-') {
        normalized.pop();
    }

    if normalized.len() > MAX_TOPIC_KEY_LEN {
        normalized.truncate(MAX_TOPIC_KEY_LEN);
        while normalized.ends_with('-') {
            normalized.pop();
        }
    }

    normalized
}

fn payload_value_as_str<'a>(event: &'a Event, key: &str) -> Option<&'a str> {
    event.payload.get(key).and_then(|value| value.as_str())
}

fn payload_value_matches(event: &Event, key: &str, expected: &str) -> bool {
    payload_value_as_str(event, key).is_some_and(|actual| same_identifier(actual, expected))
}

fn same_identifier(left: &str, right: &str) -> bool {
    left.eq(right)
}

const fn relation_kind_name(relation_kind: RelationKind) -> &'static str {
    match relation_kind {
        RelationKind::BasedOn => "BASED_ON",
        RelationKind::HasOption => "HAS_OPTION",
        RelationKind::Chose => "CHOSE",
        RelationKind::Assumes => "ASSUMES",
        RelationKind::Supports => "SUPPORTS",
        RelationKind::Refutes => "REFUTES",
    }
}

fn require_non_empty(field: &'static str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        Err(CommandError::Validation(format!("{field} must not be empty")).into())
    } else {
        Ok(())
    }
}

fn generate_entity_id(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use proptest::prelude::*;
    use serde_json::json;
    use uuid::Uuid;

    use crate::events::{EventProvenance, EventSource, EventType, RelationKind};
    use crate::ledger::{EventLedger, InMemoryEventLedger, SqliteEventLedger};

    use super::{normalize_topic_key, Commands, MAX_TOPIC_KEY_LEN};

    #[test]
    fn record_evidence_appends_evidence_recorded_event() {
        let ledger = InMemoryEventLedger::new();
        let commands = Commands::new(&ledger);

        let evidence_id = commands
            .record_evidence("actor:alice", "Observed elevated API latency")
            .expect("record evidence succeeds");

        assert!(evidence_id.starts_with("evidence-"));
        let events = ledger.read(0, 10).expect("read succeeds");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, EventType::EvidenceRecorded);
        assert_eq!(events[0].actor_id, "actor:alice");
        assert_eq!(
            events[0]
                .payload
                .get("evidence_id")
                .and_then(|value| value.as_str()),
            Some(evidence_id.as_str())
        );
    }

    #[test]
    fn record_hypothesis_appends_hypothesis_recorded_event() {
        let ledger = InMemoryEventLedger::new();
        let commands = Commands::new(&ledger);

        let hypothesis_id = commands
            .record_hypothesis(
                "actor:bob",
                "API latency spike comes from db lock contention",
            )
            .expect("record hypothesis succeeds");

        assert!(hypothesis_id.starts_with("hypothesis-"));
        let events = ledger.read(0, 10).expect("read succeeds");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, EventType::HypothesisRecorded);
        assert_eq!(events[0].actor_id, "actor:bob");
        assert_eq!(
            events[0]
                .payload
                .get("hypothesis_id")
                .and_then(|value| value.as_str()),
            Some(hypothesis_id.as_str())
        );
    }

    #[test]
    fn record_option_returns_option_id_without_writing_event() {
        let ledger = InMemoryEventLedger::new();
        let commands = Commands::new(&ledger);

        let option_id = commands
            .record_option("actor:carol", "Use queue", "Ship async queue")
            .expect("record option succeeds");

        assert!(option_id.starts_with("option-"));
        let events = ledger.read(0, 10).expect("read succeeds");
        assert!(events.is_empty());
    }

    #[test]
    fn actor_id_is_required_for_all_entity_commands() {
        let ledger = InMemoryEventLedger::new();
        let commands = Commands::new(&ledger);

        assert!(commands.record_evidence("", "content").is_err());
        assert!(commands.record_hypothesis("   ", "statement").is_err());
        assert!(commands.record_option("", "label", "description").is_err());
    }

    #[test]
    fn propose_decision_fans_out_relation_events_with_causation_linkage() {
        let ledger = InMemoryEventLedger::new();
        let commands = Commands::new(&ledger);

        let option_a = commands
            .record_option("actor:alice", "A", "Option A")
            .expect("option a");
        let option_b = commands
            .record_option("actor:alice", "B", "Option B")
            .expect("option b");
        let evidence_id = commands
            .record_evidence("actor:alice", "Log sample")
            .expect("evidence");
        let hypothesis_id = commands
            .record_hypothesis("actor:alice", "This will improve p95")
            .expect("hypothesis");

        let decision_id = commands
            .propose_decision(
                "actor:alice",
                "Pick queue strategy",
                "Need robust ingestion",
                &["Infra / Queue".to_owned()],
                &[option_a.clone(), option_b.clone()],
                Some(option_b.as_str()),
                std::slice::from_ref(&hypothesis_id),
                std::slice::from_ref(&evidence_id),
            )
            .expect("propose decision");

        let events = ledger.read(0, 20).expect("read events");
        let proposal = events
            .iter()
            .find(|event| event.event_type == EventType::DecisionProposed)
            .expect("proposal event present");
        let proposal_id = proposal.event_id.expect("proposal event id");

        let relation_events: Vec<_> = events
            .iter()
            .filter(|event| event.event_type == EventType::RelationAdded)
            .collect();
        assert_eq!(relation_events.len(), 5);

        for relation_event in &relation_events {
            assert_eq!(relation_event.causation_event_id, Some(proposal_id));
            assert_eq!(
                relation_event
                    .payload
                    .get("from_id")
                    .and_then(|value| value.as_str()),
                Some(decision_id.as_str())
            );
        }

        let has_option_count = relation_events
            .iter()
            .filter(|event| event.payload.get("relation") == Some(&json!(RelationKind::HasOption)))
            .count();
        assert_eq!(has_option_count, 2);

        let chose_count = relation_events
            .iter()
            .filter(|event| event.payload.get("relation") == Some(&json!(RelationKind::Chose)))
            .count();
        assert_eq!(chose_count, 1);
    }

    #[test]
    fn direct_agent_decision_persists_agent_provenance() {
        let dir =
            std::env::temp_dir().join(format!("hivemind-agent-provenance-{}", Uuid::new_v4()));
        let actor_id = "agent:codex:furiosa";
        let decision_id = {
            let ledger = SqliteEventLedger::open(&dir).expect("ledger opens");
            let commands = Commands::new_with_provenance(
                &ledger,
                EventProvenance::agent("agent:codex:furiosa/session-1"),
            );
            let option_id = commands
                .record_option(actor_id, "Keep substrate small", "Add source fields only")
                .expect("option recorded");
            commands
                .propose_decision(
                    actor_id,
                    "Record direct agent provenance",
                    "Agent-written decisions must be distinguishable from CLI writes",
                    &["Integrations".to_owned()],
                    &[option_id],
                    None,
                    &[],
                    &[],
                )
                .expect("agent decision proposed")
        };

        let ledger = SqliteEventLedger::open(&dir).expect("ledger reopens");
        let events = ledger.read(0, 20).expect("events read");
        let proposal = events
            .iter()
            .find(|event| {
                event.event_type == EventType::DecisionProposed
                    && event
                        .payload
                        .get("decision_id")
                        .and_then(|value| value.as_str())
                        == Some(decision_id.as_str())
            })
            .expect("proposal persisted");

        assert_eq!(proposal.actor_id, actor_id);
        assert_eq!(proposal.source, EventSource::Agent);
        assert_eq!(
            proposal.source_ref.as_deref(),
            Some("agent:codex:furiosa/session-1")
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn accept_and_reject_invariant_for_same_actor_is_enforced() {
        let ledger = InMemoryEventLedger::new();
        let commands = Commands::new(&ledger);

        let option_id = commands
            .record_option("actor:alice", "A", "Option A")
            .expect("option");
        let decision_id = commands
            .propose_decision(
                "actor:alice",
                "Pick one",
                "Need progress",
                &["Core".to_owned()],
                &[option_id],
                None,
                &[],
                &[],
            )
            .expect("propose");

        commands
            .accept_decision(&decision_id, "actor:alice")
            .expect("accept succeeds");
        assert!(commands
            .reject_decision(&decision_id, "actor:alice")
            .is_err());
    }

    #[test]
    fn supersede_requires_both_decisions_to_exist() {
        let ledger = InMemoryEventLedger::new();
        let commands = Commands::new(&ledger);

        let option_a = commands
            .record_option("actor:alice", "A", "Option A")
            .expect("option a");
        let option_b = commands
            .record_option("actor:alice", "B", "Option B")
            .expect("option b");

        let decision_a = commands
            .propose_decision(
                "actor:alice",
                "Decision A",
                "rationale",
                &["Core".to_owned()],
                &[option_a],
                None,
                &[],
                &[],
            )
            .expect("decision a");

        let decision_b = commands
            .propose_decision(
                "actor:alice",
                "Decision B",
                "rationale",
                &["Core".to_owned()],
                &[option_b],
                None,
                &[],
                &[],
            )
            .expect("decision b");

        commands
            .supersede_decision(&decision_a, &decision_b, "actor:alice")
            .expect("supersede succeeds");

        assert!(commands
            .supersede_decision("decision-missing", &decision_b, "actor:alice")
            .is_err());
    }

    #[test]
    fn attach_evidence_requires_existing_endpoints() {
        let ledger = InMemoryEventLedger::new();
        let commands = Commands::new(&ledger);

        let option_id = commands
            .record_option("actor:alice", "A", "Option A")
            .expect("option");
        let decision_id = commands
            .propose_decision(
                "actor:alice",
                "Decision A",
                "rationale",
                &["Core".to_owned()],
                &[option_id],
                None,
                &[],
                &[],
            )
            .expect("decision");
        let evidence_id = commands
            .record_evidence("actor:alice", "evidence")
            .expect("evidence");

        assert!(commands
            .attach_evidence(&decision_id, &evidence_id, "actor:alice")
            .is_ok());
        assert!(commands
            .attach_evidence("missing-decision", &evidence_id, "actor:alice")
            .is_err());
    }

    #[test]
    fn relate_evidence_to_hypothesis_requires_supports_or_refutes_and_is_idempotent() {
        let ledger = InMemoryEventLedger::new();
        let commands = Commands::new(&ledger);

        let evidence_id = commands
            .record_evidence("actor:alice", "evidence")
            .expect("evidence");
        let hypothesis_id = commands
            .record_hypothesis("actor:alice", "hypothesis")
            .expect("hypothesis");

        let first = commands
            .relate_evidence_to_hypothesis(
                &evidence_id,
                &hypothesis_id,
                RelationKind::Supports,
                "actor:alice",
            )
            .expect("first relation");
        let second = commands
            .relate_evidence_to_hypothesis(
                &evidence_id,
                &hypothesis_id,
                RelationKind::Supports,
                "actor:alice",
            )
            .expect("duplicate relation");
        assert_eq!(first, second);

        assert!(commands
            .relate_evidence_to_hypothesis(
                &evidence_id,
                &hypothesis_id,
                RelationKind::BasedOn,
                "actor:alice"
            )
            .is_err());

        let relation_events = ledger
            .read(0, 50)
            .expect("read events")
            .into_iter()
            .filter(|event| {
                event.event_type == EventType::RelationAdded
                    && event.payload.get("relation") == Some(&json!(RelationKind::Supports))
            })
            .count();
        assert_eq!(relation_events, 1);
    }

    #[test]
    fn propose_decision_normalizes_topic_keys() {
        let ledger = InMemoryEventLedger::new();
        let commands = Commands::new(&ledger);

        let option_id = commands
            .record_option("actor:alice", "A", "Option A")
            .expect("option");
        let decision_id = commands
            .propose_decision(
                "actor:alice",
                "Normalize topics",
                "Keep consistent filters",
                &[
                    "  Crème brûlée API!!  ".to_owned(),
                    "Ops___SRE   Alerts".to_owned(),
                ],
                &[option_id],
                None,
                &[],
                &[],
            )
            .expect("propose");

        let events = ledger.read(0, 10).expect("read events");
        let proposal = events
            .iter()
            .find(|event| {
                event.event_type == EventType::DecisionProposed
                    && event
                        .payload
                        .get("decision_id")
                        .and_then(|value| value.as_str())
                        == Some(decision_id.as_str())
            })
            .expect("proposal event");

        let topics = proposal
            .payload
            .get("topic_keys")
            .and_then(|value| value.as_array())
            .expect("topic keys array")
            .iter()
            .filter_map(|value| value.as_str())
            .collect::<Vec<_>>();

        assert_eq!(topics, vec!["creme-brulee-api", "ops-sre-alerts"]);
    }

    #[test]
    fn normalize_topic_key_handles_unicode_whitespace_punctuation_and_length_cap() {
        assert_eq!(
            normalize_topic_key("  Crème brûlée API!!  "),
            "creme-brulee-api"
        );
        assert_eq!(normalize_topic_key("Ops___SRE   Alerts"), "ops-sre-alerts");

        let input = "a".repeat(MAX_TOPIC_KEY_LEN + 40);
        let normalized = normalize_topic_key(&input);
        assert_eq!(normalized.len(), MAX_TOPIC_KEY_LEN);
        assert!(normalized.chars().all(|character| character == 'a'));
    }

    proptest! {
        #[test]
        fn normalize_topic_key_is_idempotent(input in ".*") {
            let normalized = normalize_topic_key(&input);
            prop_assert_eq!(normalize_topic_key(&normalized), normalized);
        }

        #[test]
        fn normalize_topic_key_outputs_ascii_slug(input in ".*") {
            let normalized = normalize_topic_key(&input);

            prop_assert!(normalized.is_ascii());
            prop_assert!(normalized.len() <= MAX_TOPIC_KEY_LEN);
            prop_assert!(!normalized.starts_with('-'));
            prop_assert!(!normalized.ends_with('-'));
            prop_assert!(normalized
                .chars()
                .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'));
            prop_assert!(!normalized.contains("--"));
        }
    }
}
