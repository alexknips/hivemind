use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

use crate::error::CommandError;
use crate::events::{Event, EventType};
use crate::ledger::EventLedger;
use crate::Result;

pub type EvidenceId = String;
pub type HypothesisId = String;
pub type OptionId = String;

pub const MAX_TOPIC_KEY_LEN: usize = 64;

pub struct Commands<'a, L: EventLedger> {
    ledger: &'a L,
}

impl<'a, L: EventLedger> Commands<'a, L> {
    pub fn new(ledger: &'a L) -> Self {
        Self { ledger }
    }

    pub fn record_evidence(&self, actor_id: &str, content: &str) -> Result<EvidenceId> {
        require_non_empty("actor_id", actor_id)?;
        require_non_empty("content", content)?;

        let evidence_id = generate_entity_id("evidence");
        let event = Event {
            event_id: None,
            event_uuid: Uuid::new_v4(),
            correlation_id: None,
            causation_event_id: None,
            event_type: EventType::EvidenceRecorded,
            actor_id: actor_id.to_owned(),
            payload: json!({
                "evidence_id": evidence_id,
                "content": content,
                "source": null
            }),
            ts: Some(Utc::now()),
        };

        self.ledger.append(event)?;
        Ok(evidence_id)
    }

    pub fn record_hypothesis(&self, actor_id: &str, statement: &str) -> Result<HypothesisId> {
        require_non_empty("actor_id", actor_id)?;
        require_non_empty("statement", statement)?;

        let hypothesis_id = generate_entity_id("hypothesis");
        let event = Event {
            event_id: None,
            event_uuid: Uuid::new_v4(),
            correlation_id: None,
            causation_event_id: None,
            event_type: EventType::HypothesisRecorded,
            actor_id: actor_id.to_owned(),
            payload: json!({
                "hypothesis_id": hypothesis_id,
                "statement": statement
            }),
            ts: Some(Utc::now()),
        };

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

        Ok(generate_entity_id("option"))
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
    use proptest::prelude::*;

    use crate::events::EventType;
    use crate::ledger::{EventLedger, InMemoryEventLedger};

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
