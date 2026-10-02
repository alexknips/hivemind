//! Which findings someone has acknowledged: the read `get_suggestions` filters by.
//!
//! Pure Layer-2 read. An acknowledgement is two recorded events: `suggestion.surfaced` makes a
//! `Notification` node carrying the `finding_id` it showed, and a later
//! `notification.acknowledged` naming that node puts `ack_at` (and `snooze_until`, `action`) on
//! it. A finding counts as acknowledged exactly when a surfaced node for its `finding_id` has an
//! `ack_at` and is not snoozed. Nothing here interprets, ranks or writes, and nothing knows what
//! a finding is: the id is an opaque string. Blocker notifications carry no `finding_id` and are
//! never read here, just as these nodes carry no `blocker_id` and are never read as blocker
//! notifications.
//!
//! One bulk read of the `Notification` nodes through `node_rows`, so memory, Postgres and Kuzu
//! agree on it.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};

use crate::projector::{GraphView, NodeKind};
use crate::Result;

use super::shared::{node_rows, optional_datetime, optional_string};

/// The `finding_id`s that are acknowledged as of `now`.
///
/// An acknowledgement that names a `snooze_until` hides the finding only until that moment: from
/// then on it no longer counts, and the finding is shown again (unless another surfaced node for
/// it is acknowledged without one). The finding's id changes when its basis does, so a finding
/// whose basis moved on since it was acknowledged is a different id and is not hidden by this.
pub fn acknowledged_finding_ids(
    graph: &impl GraphView,
    now: DateTime<Utc>,
) -> Result<BTreeSet<String>> {
    let mut acknowledged = BTreeSet::new();
    for row in node_rows(graph, NodeKind::Notification)?.values() {
        let Some(finding_id) = optional_string(row, "finding_id") else {
            continue;
        };
        if optional_string(row, "ack_at").is_none() {
            continue;
        }
        if optional_datetime(row, "snooze_until")?.is_some_and(|snooze_until| now >= snooze_until) {
            // The snooze has run out: this acknowledgement no longer hides the finding.
            continue;
        }
        acknowledged.insert(finding_id);
    }
    Ok(acknowledged)
}

#[cfg(test)]
mod tests;
