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
