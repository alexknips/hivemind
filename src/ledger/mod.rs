mod backend_error;
mod memory;
mod sqlite;

#[cfg(test)]
pub(crate) mod contract_tests;

use crate::events::{Event, EventId};
use crate::Result;

pub use memory::InMemoryEventLedger;
pub use sqlite::SqliteEventLedger;

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
