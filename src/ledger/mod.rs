use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

use chrono::Utc;
use uuid::Uuid;

use crate::error::LedgerError;
use crate::events::{Event, EventId};
use crate::Result;

pub trait EventLedger {
    fn append(&self, event: Event) -> Result<EventId>;

    fn read(&self, offset: EventId, limit: usize) -> Result<Vec<Event>>;

    fn replay_from(
        &self,
        offset: EventId,
        callback: &mut dyn FnMut(&Event) -> Result<()>,
    ) -> Result<()>;

    fn latest_offset(&self) -> Result<EventId>;
}

#[derive(Debug, Default)]
pub struct InMemoryEventLedger {
    state: Mutex<InMemoryState>,
}

#[derive(Debug, Default)]
struct InMemoryState {
    events: Vec<Event>,
    event_uuid_to_id: HashMap<Uuid, EventId>,
}

impl InMemoryEventLedger {
    pub fn new() -> Self {
        Self::default()
    }

    fn state(&self) -> Result<MutexGuard<'_, InMemoryState>> {
        self.state.lock().map_err(|error| {
            LedgerError::Storage(format!("in-memory ledger lock poisoned: {error}")).into()
        })
    }
}

impl EventLedger for InMemoryEventLedger {
    fn append(&self, mut event: Event) -> Result<EventId> {
        let mut state = self.state()?;
        if let Some(existing_id) = state.event_uuid_to_id.get(&event.event_uuid) {
            return Ok(*existing_id);
        }

        let next_id = (state.events.len() as EventId) + 1;
        event.event_id = Some(next_id);
        if event.ts.is_none() {
            event.ts = Some(Utc::now());
        }

        state.events.push(event.clone());
        state.event_uuid_to_id.insert(event.event_uuid, next_id);

        Ok(next_id)
    }

    fn read(&self, offset: EventId, limit: usize) -> Result<Vec<Event>> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let state = self.state()?;
        Ok(state
            .events
            .iter()
            .filter(|event| event.event_id.unwrap_or_default() > offset)
            .take(limit)
            .cloned()
            .collect())
    }

    fn replay_from(
        &self,
        offset: EventId,
        callback: &mut dyn FnMut(&Event) -> Result<()>,
    ) -> Result<()> {
        let replay_events = {
            let state = self.state()?;
            state
                .events
                .iter()
                .filter(|event| event.event_id.unwrap_or_default() > offset)
                .cloned()
                .collect::<Vec<_>>()
        };

        for event in &replay_events {
            callback(event)?;
        }

        Ok(())
    }

    fn latest_offset(&self) -> Result<EventId> {
        let state = self.state()?;
        Ok(state
            .events
            .last()
            .and_then(|event| event.event_id)
            .unwrap_or_default())
    }
}

#[cfg(test)]
pub(crate) mod contract_tests {
    use chrono::Utc;
    use serde_json::json;
    use uuid::Uuid;

    use crate::events::{Event, EventId, EventType};
    use crate::ledger::EventLedger;
    use crate::Result;

    pub fn assert_monotonic_append<L: EventLedger>(ledger: &L) -> Result<()> {
        let event_ids = [
            ledger.append(make_event("evidence-1", Uuid::new_v4()))?,
            ledger.append(make_event("evidence-2", Uuid::new_v4()))?,
            ledger.append(make_event("evidence-3", Uuid::new_v4()))?,
        ];

        assert_eq!(event_ids, [1, 2, 3]);
        assert_eq!(ledger.latest_offset()?, 3);
        Ok(())
    }

    pub fn assert_dedup_by_event_uuid<L: EventLedger>(ledger: &L) -> Result<()> {
        let duplicate_uuid = Uuid::new_v4();

        let first_id = ledger.append(make_event("evidence-original", duplicate_uuid))?;
        let second_id = ledger.append(make_event("evidence-ignored", duplicate_uuid))?;

        assert_eq!(first_id, 1);
        assert_eq!(second_id, 1);
        assert_eq!(ledger.latest_offset()?, 1);

        let events = ledger.read(0, 10)?;
        assert_eq!(events.len(), 1);
        assert_eq!(event_id_from_payload(&events[0]), "evidence-original");

        Ok(())
    }

    pub fn assert_replay_from_zero_in_order<L: EventLedger>(ledger: &L) -> Result<()> {
        ledger.append(make_event("evidence-a", Uuid::new_v4()))?;
        ledger.append(make_event("evidence-b", Uuid::new_v4()))?;
        ledger.append(make_event("evidence-c", Uuid::new_v4()))?;

        let mut replayed_ids = Vec::new();
        ledger.replay_from(0, &mut |event| {
            replayed_ids.push(event.event_id.unwrap_or_default());
            Ok(())
        })?;

        assert_eq!(replayed_ids, vec![1, 2, 3]);

        Ok(())
    }

    pub fn assert_read_offset_and_limit<L: EventLedger>(ledger: &L) -> Result<()> {
        ledger.append(make_event("evidence-a", Uuid::new_v4()))?;
        ledger.append(make_event("evidence-b", Uuid::new_v4()))?;
        ledger.append(make_event("evidence-c", Uuid::new_v4()))?;

        let events = ledger.read(1, 1)?;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_id, Some(2));

        let events = ledger.read(3, 5)?;
        assert!(events.is_empty());

        Ok(())
    }

    fn make_event(evidence_id: &str, event_uuid: Uuid) -> Event {
        Event {
            event_id: None,
            event_uuid,
            correlation_id: None,
            causation_event_id: None,
            event_type: EventType::EvidenceRecorded,
            actor_id: "actor:test".to_owned(),
            payload: json!({
                "evidence_id": evidence_id,
                "content": format!("content for {evidence_id}"),
                "source": "unit-test"
            }),
            ts: Some(Utc::now()),
        }
    }

    fn event_id_from_payload(event: &Event) -> &str {
        event
            .payload
            .get("evidence_id")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
    }

    #[allow(dead_code)]
    fn _assert_event_ids_are_monotonic(ids: &[EventId]) {
        let mut previous = 0;
        for id in ids {
            assert!(*id > previous);
            previous = *id;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::ledger::contract_tests::{
        assert_dedup_by_event_uuid, assert_monotonic_append, assert_read_offset_and_limit,
        assert_replay_from_zero_in_order,
    };

    use super::InMemoryEventLedger;
    use crate::Result;

    #[test]
    fn in_memory_append_assigns_monotonic_ids() -> Result<()> {
        let ledger = InMemoryEventLedger::new();
        assert_monotonic_append(&ledger)
    }

    #[test]
    fn in_memory_append_is_idempotent_for_duplicate_event_uuid() -> Result<()> {
        let ledger = InMemoryEventLedger::new();
        assert_dedup_by_event_uuid(&ledger)
    }

    #[test]
    fn in_memory_replay_from_zero_is_ordered() -> Result<()> {
        let ledger = InMemoryEventLedger::new();
        assert_replay_from_zero_in_order(&ledger)
    }

    #[test]
    fn in_memory_read_applies_offset_and_limit() -> Result<()> {
        let ledger = InMemoryEventLedger::new();
        assert_read_offset_and_limit(&ledger)
    }
}
