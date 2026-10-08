//! Ledger trait and backend selection: append-only event store backed by SQLite, in-memory, or Postgres.

mod any;
mod backend_error;
mod memory;
#[cfg(feature = "shared-backend-postgres")]
mod postgres;
mod sqlite;

#[cfg(test)]
pub(crate) mod contract_tests;

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::events::{Event, EventId, EventType, TenantId};
use crate::Result;

pub use any::{AnyLedger, LedgerConfig};
pub use memory::InMemoryEventLedger;
#[cfg(feature = "shared-backend-postgres")]
pub use postgres::{PostgresEventLedger, ProvisionedUser, ResolvedToken, TenantStore, UserInfo};
pub use sqlite::{SqliteEventLedger, SqliteUserStore, SQLITE_TOKEN_PREFIX};

/// An event reduced to what a caller named: its id, time and actor, and the string-valued
/// top-level payload keys it asked for. What [`EventLedger::read_fields_for_tenant`] returns.
#[derive(Debug, Clone, PartialEq)]
pub struct EventFields {
    pub event_id: EventId,
    pub ts: Option<DateTime<Utc>>,
    pub actor_id: String,
    /// One entry per requested key, in the order asked: the string the payload holds under it,
    /// or `None` when the key is absent or holds anything but a string.
    pub fields: Vec<Option<String>>,
}

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

    /// Events of the given `types` after `offset`, oldest first, at most `limit`, each with the
    /// top-level payload keys named in `omit_payload_keys` left out. For a caller that needs one
    /// kind of event, or only the small fields of a large one, without reading the whole ledger:
    /// the SQL backends filter and strip in the query, so only what was asked for crosses the
    /// wire. The default pages through [`Self::read_for_tenant`] and does the same in memory:
    /// always correct, never faster than a full scan.
    fn read_types_for_tenant(
        &self,
        tenant_id: &TenantId,
        types: &[EventType],
        omit_payload_keys: &[&str],
        offset: EventId,
        limit: usize,
    ) -> Result<Vec<Event>> {
        const PAGE: usize = 1024;
        let mut found = Vec::new();
        let mut cursor = offset;
        while found.len() < limit {
            let page = self.read_for_tenant(tenant_id, cursor, PAGE)?;
            let Some(last) = page.last().and_then(|event| event.event_id) else {
                break;
            };
            cursor = last;
            for mut event in page {
                if found.len() < limit && types.contains(&event.event_type) {
                    omit_payload_keys_from(&mut event, omit_payload_keys);
                    found.push(event);
                }
            }
        }
        Ok(found)
    }

    /// Events of one `event_type` after `offset`, oldest first, at most `limit`, reduced to
    /// [`EventFields`]: the cheapest read of a few small fields from many events. The Postgres
    /// ledger extracts the fields in the query and sends only them; the default reads the typed
    /// events and picks the fields in memory (always correct, and fast on a local ledger).
    fn read_fields_for_tenant(
        &self,
        tenant_id: &TenantId,
        event_type: EventType,
        payload_keys: &[&str],
        offset: EventId,
        limit: usize,
    ) -> Result<Vec<EventFields>> {
        let events = self.read_types_for_tenant(tenant_id, &[event_type], &[], offset, limit)?;
        Ok(events
            .into_iter()
            .filter_map(|event| {
                Some(EventFields {
                    event_id: event.event_id?,
                    ts: event.ts,
                    actor_id: event.actor_id,
                    fields: payload_keys
                        .iter()
                        .map(|key| {
                            event
                                .payload
                                .get(*key)
                                .and_then(|value| value.as_str())
                                .map(str::to_owned)
                        })
                        .collect(),
                })
            })
            .collect())
    }

    /// The events with exactly these ids, oldest first. An id that names no event of the tenant
    /// is skipped, not an error.
    fn read_ids_for_tenant(
        &self,
        tenant_id: &TenantId,
        event_ids: &[EventId],
    ) -> Result<Vec<Event>> {
        let mut ids = event_ids.to_vec();
        ids.sort_unstable();
        ids.dedup();
        let mut found = Vec::with_capacity(ids.len());
        for id in ids {
            let read = self.read_for_tenant(tenant_id, id.saturating_sub(1), 1)?;
            found.extend(read.into_iter().filter(|event| event.event_id == Some(id)));
        }
        Ok(found)
    }

    /// The event id each of these `uuids` has in the tenant. A uuid the tenant holds no event
    /// for is absent from the map, not an error. What a replay needs to tell an event the
    /// destination already holds from a new one, and to number a causation link by the cause's
    /// uuid. The default scans the whole tenant; the SQL backends look the uuids up by index.
    fn event_ids_for_uuids_for_tenant(
        &self,
        tenant_id: &TenantId,
        uuids: &[Uuid],
    ) -> Result<HashMap<Uuid, EventId>> {
        let wanted: HashSet<Uuid> = uuids.iter().copied().collect();
        let mut found = HashMap::new();
        if wanted.is_empty() {
            return Ok(found);
        }
        self.replay_from_for_tenant(tenant_id, 0, &mut |event| {
            if let (true, Some(event_id)) = (wanted.contains(&event.event_uuid), event.event_id) {
                found.insert(event.event_uuid, event_id);
            }
            Ok(())
        })?;
        Ok(found)
    }

    /// The tenant the plain methods below (`append`, `read`, `replay_from`, `latest_offset`)
    /// address: `local`, unless the ledger is bound to another tenant (the Postgres ledger opened
    /// for one). A caller that names the tenant itself uses the `_for_tenant` methods; one that
    /// takes "the ledger's own events" and needs `_for_tenant` reads too asks this.
    fn bound_tenant(&self) -> TenantId {
        TenantId::local()
    }

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

/// Removes the top-level payload keys `keys` from `event`. A payload that is not a JSON object
/// has no keys to remove.
fn omit_payload_keys_from(event: &mut Event, keys: &[&str]) {
    if let Some(payload) = event.payload.as_object_mut() {
        for key in keys {
            payload.remove(*key);
        }
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

    fn read_types_for_tenant(
        &self,
        _tenant_id: &TenantId,
        types: &[EventType],
        omit_payload_keys: &[&str],
        offset: EventId,
        limit: usize,
    ) -> Result<Vec<Event>> {
        self.ledger
            .read_types_for_tenant(&self.tenant_id, types, omit_payload_keys, offset, limit)
    }

    fn read_ids_for_tenant(
        &self,
        _tenant_id: &TenantId,
        event_ids: &[EventId],
    ) -> Result<Vec<Event>> {
        self.ledger.read_ids_for_tenant(&self.tenant_id, event_ids)
    }

    fn event_ids_for_uuids_for_tenant(
        &self,
        _tenant_id: &TenantId,
        uuids: &[Uuid],
    ) -> Result<HashMap<Uuid, EventId>> {
        self.ledger
            .event_ids_for_uuids_for_tenant(&self.tenant_id, uuids)
    }

    fn read_fields_for_tenant(
        &self,
        _tenant_id: &TenantId,
        event_type: EventType,
        payload_keys: &[&str],
        offset: EventId,
        limit: usize,
    ) -> Result<Vec<EventFields>> {
        self.ledger
            .read_fields_for_tenant(&self.tenant_id, event_type, payload_keys, offset, limit)
    }
}

/// Like [`TenantScopedLedger`] but owns its base ledger instead of
/// borrowing it. `TenantScopedLedger` needs a `&'a L` that outlives its own
/// use, which doesn't fit a resolver closure that opens a fresh ledger
/// value per call and must return it (e.g. the Slack multi-tenant drain
/// loop in `api::slack`, which resolves one tenant-scoped ledger per queued
/// capture). Same override behavior: every `EventLedger` call is pinned to
/// `tenant_id` regardless of what the caller passes.
#[derive(Debug)]
pub struct TenantScopedOwnedLedger<L: EventLedger> {
    ledger: L,
    tenant_id: TenantId,
}

impl<L: EventLedger> TenantScopedOwnedLedger<L> {
    pub fn new(ledger: L, tenant_id: TenantId) -> Self {
        Self { ledger, tenant_id }
    }
}

impl<L: EventLedger> EventLedger for TenantScopedOwnedLedger<L> {
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

    fn read_types_for_tenant(
        &self,
        _tenant_id: &TenantId,
        types: &[EventType],
        omit_payload_keys: &[&str],
        offset: EventId,
        limit: usize,
    ) -> Result<Vec<Event>> {
        self.ledger
            .read_types_for_tenant(&self.tenant_id, types, omit_payload_keys, offset, limit)
    }

    fn read_ids_for_tenant(
        &self,
        _tenant_id: &TenantId,
        event_ids: &[EventId],
    ) -> Result<Vec<Event>> {
        self.ledger.read_ids_for_tenant(&self.tenant_id, event_ids)
    }

    fn event_ids_for_uuids_for_tenant(
        &self,
        _tenant_id: &TenantId,
        uuids: &[Uuid],
    ) -> Result<HashMap<Uuid, EventId>> {
        self.ledger
            .event_ids_for_uuids_for_tenant(&self.tenant_id, uuids)
    }

    fn read_fields_for_tenant(
        &self,
        _tenant_id: &TenantId,
        event_type: EventType,
        payload_keys: &[&str],
        offset: EventId,
        limit: usize,
    ) -> Result<Vec<EventFields>> {
        self.ledger
            .read_fields_for_tenant(&self.tenant_id, event_type, payload_keys, offset, limit)
    }
}
