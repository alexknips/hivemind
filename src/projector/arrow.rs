//! Arrow orientation: every edge is *drawn* from the newer node to the older node.
//!
//! The projector stores each relation in one fixed, semantic direction (the side making the
//! claim is the source: a decision is `BASED_ON` its evidence, evidence `SUPPORTS` a
//! hypothesis). Every query walks that stored direction, so no read has to look in two places.
//! What a reader is *shown* is a different thing: an [`Arrow`]. It runs from the node that was
//! recorded later to the node recorded earlier, whichever way the relation is stored, and its
//! label reads correctly along that direction ("based on" one way, "informs" the other).
//!
//! [`orient`] is the only place that decision is made. It is pure: two node times in, one arrow
//! out. Every read surface that returns edges (`GET /v1/graph`, the neighborhood query, the CLI
//! and TUI renderers) goes through it, so the server and the UI cannot disagree about a
//! direction. See `docs/GRAPH_CONTRACT.md`.

use serde::Serialize;

use super::{NodeKind, RelationKind};

/// One edge, oriented newer → older, with its label in that direction.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Arrow {
    /// The relation as stored. It names the *meaning* of the edge; it does not say which end is
    /// the source once [`Arrow::reversed`] is set.
    pub relation: RelationKind,
    pub from_kind: NodeKind,
    pub from_id: String,
    pub to_kind: NodeKind,
    pub to_id: String,
    /// The relation read along this arrow, an active phrase: `based on`, `informs`.
    pub label: &'static str,
    /// True when this arrow runs against the stored direction because the stored target was
    /// recorded after the stored source.
    pub reversed: bool,
}

impl Arrow {
    /// The stored (semantic) source of the edge: the side making the claim.
    pub fn stored_source(&self) -> &str {
        if self.reversed {
            &self.to_id
        } else {
            &self.from_id
        }
    }

    /// The stored (semantic) target of the edge.
    pub fn stored_target(&self) -> &str {
        if self.reversed {
            &self.from_id
        } else {
            &self.to_id
        }
    }
}

/// How one relation reads in each direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArrowLabels {
    /// Label when the arrow runs as stored: the stored source is the newer node (or the same
    /// age).
    pub forward: &'static str,
    /// Label when the arrow runs against the stored direction: the stored target is the newer
    /// node. `None` when the target can never be newer, because it is an [`NodeKind::Actor`].
    pub reversed: Option<&'static str>,
}

impl RelationKind {
    /// The labels this relation carries along an arrow, in each direction.
    pub const fn arrow_labels(self) -> ArrowLabels {
        match self {
            Self::ProposedBy | Self::RequestProposedBy => ArrowLabels::anchored("proposed by"),
            Self::AcceptedBy | Self::RequestAcceptedBy => ArrowLabels::anchored("accepted by"),
            Self::RejectedBy | Self::RequestRejectedBy => ArrowLabels::anchored("rejected by"),
            Self::DecisionRequestedBy => ArrowLabels::anchored("requested by"),
            Self::DecisionRequestRequiredOwner | Self::BlockerRequiredOwner => {
                ArrowLabels::anchored("waits on")
            }
            Self::BlockedActor => ArrowLabels::anchored("blocks"),
            Self::NotificationRecipient => ArrowLabels::anchored("is sent to"),
            Self::ParticipatedBy => ArrowLabels::anchored("captured with"),
            Self::InitiatedBy => ArrowLabels::anchored("initiated by"),
            Self::DecisionRequestForDecision => ArrowLabels::both("asks about", "answers"),
            Self::BlockerForDecision => ArrowLabels::both("waits on", "unblocks"),
            Self::NotificationForBlocker => ArrowLabels::both("is about", "is announced by"),
            Self::Supersedes => ArrowLabels::both("supersedes", "is superseded by"),
            Self::SameAs => ArrowLabels::both("is the same as", "is the same as"),
            Self::HasOption => ArrowLabels::both("weighs", "is weighed in"),
            Self::Chose => ArrowLabels::both("chose", "is chosen in"),
            Self::BasedOn => ArrowLabels::both("based on", "informs"),
            Self::PremisedOn | Self::PremisedOnDirect => ArrowLabels::both("rests on", "underpins"),
            Self::Supports => ArrowLabels::both("supports", "draws on"),
            Self::Refutes => ArrowLabels::both("refutes", "is refuted by"),
            Self::FollowsFrom => ArrowLabels::both("follows from", "underlies"),
            Self::PartOf => ArrowLabels::both("is part of", "contains"),
            Self::DependsOn => ArrowLabels::both("depends on", "is needed by"),
        }
    }
}

impl ArrowLabels {
    const fn anchored(forward: &'static str) -> Self {
        Self {
            forward,
            reversed: None,
        }
    }

    const fn both(forward: &'static str, reversed: &'static str) -> Self {
        Self {
            forward,
            reversed: Some(reversed),
        }
    }
}

/// Where a node sits on the ledger's time axis: the offset of the event that created it.
/// `None` when the node carries none (a placeholder from a store that predates `event_origin`).
pub type NodeTime = Option<i64>;

/// Orients one stored edge `stored_from -[relation]-> stored_to` newer → older.
///
/// - An [`NodeKind::Actor`] is an identity, not a record: it has no place on the time axis and
///   counts as older than every record. An edge to an actor therefore never reverses.
/// - The arrow reverses only when the stored target was recorded strictly after the stored
///   source. Equal times (both nodes made by the same event) and unknown times keep the stored
///   direction, so an edge never flips on missing data.
pub fn orient(
    relation: RelationKind,
    stored_from: &str,
    stored_from_time: NodeTime,
    stored_to: &str,
    stored_to_time: NodeTime,
) -> Arrow {
    let (from_kind, to_kind) = relation.endpoints();
    let labels = relation.arrow_labels();
    let reversed_label = labels.reversed.filter(
        |_| matches!((stored_from_time, stored_to_time), (Some(from), Some(to)) if from < to),
    );
    if let Some(label) = reversed_label {
        Arrow {
            relation,
            from_kind: to_kind,
            from_id: stored_to.to_owned(),
            to_kind: from_kind,
            to_id: stored_from.to_owned(),
            label,
            reversed: true,
        }
    } else {
        Arrow {
            relation,
            from_kind,
            from_id: stored_from.to_owned(),
            to_kind,
            to_id: stored_to.to_owned(),
            label: labels.forward,
            reversed: false,
        }
    }
}
