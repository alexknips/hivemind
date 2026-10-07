//! Restatements: a decision stated again is one decision, not two (hivemind-83cj).
//!
//! Every session that relays or re-reads a ruling can classify it into a fresh decision. The
//! classifier (layer 3) judges, from the closest recorded decisions it was shown, that a
//! capture restates one of them and says so with `CaptureItem::restates_id`. This module is the
//! write layer's half, and it never judges: it checks the named id and then applies one
//! mechanical rule. The invariants are listed in the `commands` module header.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::error::CommandError;
use crate::events::{
    classified_batch_ids, received_batch_newest_turn_time, CaptureItem, EventId, EventType,
    RelationKind,
};
use crate::ledger::EventLedger;
use crate::util::{require_non_empty, require_valid_actor_id};
use crate::Result;

use super::{classified_capture_position, payload_value_as_str, Commands};

/// What became of a capture that named a decision it restates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RestatementOutcome {
    /// Recorded as its own decision, linked `SAME_AS` to the one it restates: the decision was
    /// made again at another moment.
    Linked,
    /// Not recorded: the same moment, seen again (a re-ingested transcript).
    Deduplicated,
}

/// One capture that named a decision it restates, and what became of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RestatedCapture {
    /// Position among the captures as submitted (a deduplicated capture is not recorded, so the
    /// recorded positions after it shift).
    pub index: usize,
    pub title: String,
    pub restates_id: String,
    pub outcome: RestatementOutcome,
}

/// What `Commands::record_ingest_batch_classified` recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassifiedBatchRecorded {
    pub event_id: EventId,
    /// Captures in the event: those submitted, less the deduplicated ones.
    pub recorded_count: usize,
    /// Every capture that named a decision it restates, in submission order.
    pub restated: Vec<RestatedCapture>,
}

impl ClassifiedBatchRecorded {
    /// Adds `restated` to a reply object (each capture that named a decision it restates, and
    /// whether it was `linked` or `deduplicated`) when any did, and nothing otherwise, so every
    /// surface (CLI, HTTP, MCP) says the same thing in the same place.
    pub fn annotate_reply(&self, reply: &mut serde_json::Value) {
        if self.restated.is_empty() {
            return;
        }
        if let (Some(reply), Ok(restated)) =
            (reply.as_object_mut(), serde_json::to_value(&self.restated))
        {
            reply.insert("restated".to_owned(), restated);
        }
    }
}

impl<L: EventLedger> Commands<'_, L> {
    /// Applies the restatement rules to the captures of one classification covering `batch_ids`:
    /// returns the captures to record and one entry per capture that named a decision it
    /// restates. Refuses, with nothing recorded, when a `restates_id` is on anything but a
    /// decision or names a decision that is not recorded.
    pub(super) fn settle_restatements(
        &self,
        batch_ids: &[String],
        captures: Vec<CaptureItem>,
    ) -> Result<(Vec<CaptureItem>, Vec<RestatedCapture>)> {
        if captures.iter().all(|capture| capture.restates_id.is_none()) {
            return Ok((captures, Vec::new()));
        }

        for (index, capture) in captures.iter().enumerate() {
            let Some(restated_id) = &capture.restates_id else {
                continue;
            };
            if capture.kind != "decision" {
                return Err(CommandError::Validation(format!(
                    "capture {index} is a {}, and only a decision can restate a decision: leave restates_id out",
                    capture.kind
                ))
                .into());
            }
            require_non_empty("restates_id", restated_id)?;
            if self.decision_recorded_text(restated_id)?.is_none() {
                return Err(CommandError::Validation(format!(
                    "capture {index} restates decision {restated_id}, which is not recorded: name an existing decision id, or leave restates_id out"
                ))
                .into());
            }
        }

        let same_moment = self.same_moment_flags(batch_ids, &captures)?;

        let mut kept = Vec::with_capacity(captures.len());
        let mut restated = Vec::new();
        // Title of a capture not recorded -> the decision it restates, for the references
        // other captures of this response made to it by title.
        let mut stood_in_for: HashMap<String, String> = HashMap::new();
        for (index, (capture, same_moment)) in captures.into_iter().zip(same_moment).enumerate() {
            let Some(restates_id) = capture.restates_id.clone() else {
                kept.push(capture);
                continue;
            };
            restated.push(RestatedCapture {
                index,
                title: capture.title.clone(),
                restates_id: restates_id.clone(),
                outcome: if same_moment {
                    RestatementOutcome::Deduplicated
                } else {
                    RestatementOutcome::Linked
                },
            });
            if same_moment {
                stood_in_for.insert(capture.title.trim().to_owned(), restates_id);
            } else {
                kept.push(capture);
            }
        }
        if !stood_in_for.is_empty() {
            for capture in &mut kept {
                point_references_at_restated(capture, &stood_in_for);
            }
        }
        Ok((kept, restated))
    }

    /// For each capture, whether it restates, from the very same moment, the decision it names.
    ///
    /// Two decisions are from the same moment when both were classified from turns whose newest
    /// source time (`IngestTurn::ts`) is the same instant: a re-ingested transcript carries the
    /// same turn times, a decision made again later does not. Only a classified capture has a
    /// source time. A restated decision recorded any other way, or either side's turns carrying
    /// no time, is never the same moment: the capture is recorded and linked, so nothing is
    /// ever dropped on a guess.
    fn same_moment_flags(
        &self,
        batch_ids: &[String],
        captures: &[CaptureItem],
    ) -> Result<Vec<bool>> {
        // The batches the restated decisions were classified from, by restated decision id.
        let mut restated_batches: HashMap<&str, Vec<String>> = HashMap::new();
        for restated_id in captures
            .iter()
            .filter_map(|capture| capture.restates_id.as_deref())
        {
            if restated_batches.contains_key(restated_id) {
                continue;
            }
            if let Some(batches) = self.classified_capture_batches(restated_id)? {
                restated_batches.insert(restated_id, batches);
            }
        }

        let mut wanted: HashSet<&str> = batch_ids.iter().map(String::as_str).collect();
        wanted.extend(restated_batches.values().flatten().map(String::as_str));
        let newest_turn = self.newest_turn_time_by_batch(&wanted)?;
        let source_time = |batches: &[String]| -> Option<DateTime<Utc>> {
            batches
                .iter()
                .filter_map(|batch_id| newest_turn.get(batch_id.as_str()).copied())
                .max()
        };

        let this_moment = source_time(batch_ids);
        Ok(captures
            .iter()
            .map(|capture| {
                let Some(restated_id) = capture.restates_id.as_deref() else {
                    return false;
                };
                let restated_moment = restated_batches
                    .get(restated_id)
                    .and_then(|batches| source_time(batches));
                this_moment.is_some() && this_moment == restated_moment
            })
            .collect())
    }

    /// The batches a classified capture (`capture:<event>:<index>`) was classified from, or
    /// `None` when `decision_id` is not one.
    fn classified_capture_batches(&self, decision_id: &str) -> Result<Option<Vec<String>>> {
        let Some((batch_event_id, _index)) = classified_capture_position(decision_id) else {
            return Ok(None);
        };
        Ok(self
            .ledger
            .read_for_tenant(&self.context.tenant_id, batch_event_id - 1, 1)?
            .into_iter()
            .next()
            .filter(|event| {
                event.event_id == Some(batch_event_id)
                    && event.event_type == EventType::IngestBatchClassified
            })
            .map(|event| classified_batch_ids(&event.payload)))
    }

    /// For each of `wanted` that was received with at least one dated turn, the newest turn time.
    fn newest_turn_time_by_batch(
        &self,
        wanted: &HashSet<&str>,
    ) -> Result<HashMap<String, DateTime<Utc>>> {
        let mut newest: HashMap<String, DateTime<Utc>> = HashMap::new();
        if wanted.is_empty() {
            return Ok(newest);
        }
        self.for_each_event(|event| {
            if event.event_type != EventType::IngestBatchReceived {
                return;
            }
            let Some(batch_id) = payload_value_as_str(event, "batch_id") else {
                return;
            };
            if !wanted.contains(batch_id) {
                return;
            }
            if let Some(batch_newest) = received_batch_newest_turn_time(&event.payload) {
                newest
                    .entry(batch_id.to_owned())
                    .and_modify(|current| *current = (*current).max(batch_newest))
                    .or_insert(batch_newest);
            }
        })?;
        Ok(newest)
    }

    /// Refuses, writing nothing, a `SAME_AS` link `link_same_as` would refuse: a blank id, a
    /// decision linked to itself, or an id that is not a recorded decision.
    pub fn require_linkable(&self, decision_id: &str, restated_id: &str) -> Result<()> {
        require_non_empty("decision_id", decision_id)?;
        require_non_empty("restated_id", restated_id)?;
        if decision_id == restated_id {
            return Err(CommandError::Validation(format!(
                "decision {decision_id} cannot be the same decision as itself"
            ))
            .into());
        }
        for id in [decision_id, restated_id] {
            if self.decision_recorded_text(id)?.is_none() {
                return Err(
                    CommandError::Validation(format!("decision is not recorded: {id}")).into(),
                );
            }
        }
        Ok(())
    }

    /// Records that `decision_id` is the same decision as `restated_id`, as `relation.added
    /// SAME_AS` (newer to older), attributed to `actor_id`. Both must be recorded decisions.
    /// Returns `None`, writing nothing, when the two are already linked either way round.
    ///
    /// This is a link, not a merge: both decisions stay as recorded, and nothing is deleted or
    /// rewritten. Reads fold them into one (`queries::same_as`).
    pub fn link_same_as(
        &self,
        actor_id: &str,
        decision_id: &str,
        restated_id: &str,
    ) -> Result<Option<EventId>> {
        require_valid_actor_id(actor_id)?;
        self.require_linkable(decision_id, restated_id)?;
        if self
            .find_relation_event_id(RelationKind::SameAs, decision_id, restated_id)?
            .or(self.find_relation_event_id(RelationKind::SameAs, restated_id, decision_id)?)
            .is_some()
        {
            return Ok(None);
        }
        self.append_relation_event_with_uuid(
            actor_id,
            0,
            RelationKind::SameAs,
            decision_id,
            restated_id,
            uuid::Uuid::new_v4(),
        )
        .map(Some)
    }
}

/// A capture's relational ids may name a sibling by title (see the classifier prompt). When that
/// sibling was not recorded because it restated an earlier decision, the reference names that
/// decision instead, so it still points at something.
fn point_references_at_restated(capture: &mut CaptureItem, stood_in_for: &HashMap<String, String>) {
    let repoint = |reference: &mut String| {
        if let Some(restated_id) = stood_in_for.get(reference.trim()) {
            restated_id.clone_into(reference);
        }
    };
    if let Some(supersedes_id) = capture.supersedes_id.as_mut() {
        repoint(supersedes_id);
    }
    for reference in capture
        .evidence_ids
        .iter_mut()
        .chain(capture.premised_on_ids.iter_mut())
        .chain(capture.supports_ids.iter_mut())
        .chain(capture.refutes_ids.iter_mut())
    {
        repoint(reference);
    }
}
