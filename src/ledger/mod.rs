mod backend_error;
mod memory;
#[cfg(feature = "shared-backend-postgres")]
mod postgres;
mod sqlite;

#[cfg(test)]
pub(crate) mod contract_tests;

use crate::events::{Event, EventId, TenantId};
use crate::Result;

pub use memory::InMemoryEventLedger;
#[cfg(feature = "shared-backend-postgres")]
pub use postgres::PostgresEventLedger;
pub use sqlite::SqliteEventLedger;

pub trait EventLedger {
    fn append_for_tenant(&self, tenant_id: &TenantId, event: Event) -> Result<EventId>;

    fn read_for_tenant(
        &self,
        tenant_id: &TenantId,
        offset: EventId,
        limit: usize,
    ) -> Result<Vec<Event>>;

    fn replay_from_for_tenant(
        &self,
        tenant_id: &TenantId,
        offset: EventId,
        callback: &mut dyn FnMut(&Event) -> Result<()>,
    ) -> Result<()>;

    fn latest_offset_for_tenant(&self, tenant_id: &TenantId) -> Result<EventId>;

    fn append(&self, event: Event) -> Result<EventId> {
        self.append_for_tenant(&TenantId::local(), event)
    }

    fn read(&self, offset: EventId, limit: usize) -> Result<Vec<Event>> {
        self.read_for_tenant(&TenantId::local(), offset, limit)
    }

    fn replay_from(
        &self,
        offset: EventId,
        callback: &mut dyn FnMut(&Event) -> Result<()>,
    ) -> Result<()> {
        self.replay_from_for_tenant(&TenantId::local(), offset, callback)
    }

    fn latest_offset(&self) -> Result<EventId> {
        self.latest_offset_for_tenant(&TenantId::local())
    }
}

#[derive(Debug)]
pub struct TenantScopedLedger<'a, L: EventLedger + ?Sized> {
    ledger: &'a L,
    tenant_id: TenantId,
}

impl<'a, L: EventLedger + ?Sized> TenantScopedLedger<'a, L> {
    pub fn new(ledger: &'a L, tenant_id: TenantId) -> Self {
        Self { ledger, tenant_id }
    }

    pub fn tenant_id(&self) -> &TenantId {
        &self.tenant_id
    }
}

impl<L: EventLedger + ?Sized> EventLedger for TenantScopedLedger<'_, L> {
    fn append_for_tenant(&self, _tenant_id: &TenantId, event: Event) -> Result<EventId> {
        self.ledger.append_for_tenant(&self.tenant_id, event)
    }

    fn read_for_tenant(
        &self,
        _tenant_id: &TenantId,
        offset: EventId,
        limit: usize,
    ) -> Result<Vec<Event>> {
        self.ledger.read_for_tenant(&self.tenant_id, offset, limit)
    }

    fn replay_from_for_tenant(
        &self,
        _tenant_id: &TenantId,
        offset: EventId,
        callback: &mut dyn FnMut(&Event) -> Result<()>,
    ) -> Result<()> {
        self.ledger
            .replay_from_for_tenant(&self.tenant_id, offset, callback)
    }

    fn latest_offset_for_tenant(&self, _tenant_id: &TenantId) -> Result<EventId> {
        self.ledger.latest_offset_for_tenant(&self.tenant_id)
    }
}
