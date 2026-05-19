use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

use chrono::Utc;
use uuid::Uuid;

use crate::events::{Event, EventId};
use crate::Result;

use super::backend_error::storage_error;
use super::EventLedger;

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
        Ok(self.state.lock().map_err(storage_error)?)
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
mod tests {
    use crate::ledger::contract_tests::{
        assert_dedup_by_event_uuid, assert_monotonic_append, assert_read_offset_and_limit,
        assert_replay_from_zero_in_order,
    };
    use crate::Result;

    use super::InMemoryEventLedger;

    #[test]
    fn append_assigns_monotonic_ids() -> Result<()> {
        let ledger = InMemoryEventLedger::new();
        assert_monotonic_append(&ledger)
    }

    #[test]
    fn append_is_idempotent_for_duplicate_event_uuid() -> Result<()> {
        let ledger = InMemoryEventLedger::new();
        assert_dedup_by_event_uuid(&ledger)
    }

    #[test]
    fn replay_from_zero_is_ordered() -> Result<()> {
        let ledger = InMemoryEventLedger::new();
        assert_replay_from_zero_in_order(&ledger)
    }

    #[test]
    fn read_applies_offset_and_limit() -> Result<()> {
        let ledger = InMemoryEventLedger::new();
        assert_read_offset_and_limit(&ledger)
    }
}
