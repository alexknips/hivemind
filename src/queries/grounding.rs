//! What a decision rests on ("grounding"), read from the graph. Design: hivemind-zdsh.15 §5.
//!
//! A decision rests on a prior decision (`FOLLOWS_FROM`), an observation (`BASED_ON`), an
//! assumption (`ASSUMES` to a hypothesis of kind `assumption`) or a declared bet (`ASSUMES` to a
//! hypothesis of kind `bet`). Each kind has its own later question — does the prior decision
//! still hold? is the observation still true? did the assumption hold? did we check the bet? —
//! and every item says whether it was named at capture or attributed later, never blended.
//!
//! Pure Layer-2 reads: no writes, no ranking, no LLM. The one input that is not in the graph is
//! the clock, needed to say a bet's check date has passed; it is read at the public entry points
//! only, and every `_at` variant takes it explicitly so tests stay deterministic.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::events::HypothesisKind;
use crate::projector::{GraphParams, GraphRow, GraphValue, GraphView, NodeKind, RelationKind};
use crate::Result;

use super::shared::{
    neighbor_ids, neighbor_pairs, node_row, optional_datetime, optional_int, optional_string,
    premised_on_hypothesis_ids, query_superseder, required_string, Direction,
};
use super::status::{
    derive_decision_status, derive_hypothesis_status, DecisionStatus, HypothesisStatus,
};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// What kind of thing a decision rests on. Each has a different later question.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GroundingKind {
    Decision,
    Evidence,
    Assumption,
    Bet,
}

/// The decision's overall grounding, derived from its edges.
///
/// `NothingDeclared` means "never asked" (legacy records, classifier extraction, document
/// import, a raw `emit decision.proposed`), which is not the same as "there was nothing": a
/// declared bet is `Bet`, and that is a positive answer.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GroundingState {
    /// At least one prior decision, observation or assumption.
    Grounded,
    /// No grounded item, at least one declared bet.
    Bet,
    /// No grounding edge of any kind.
    NothingDeclared,
}

/// The current state of one thing a decision rests on. `state` is the wire tag; the same name
/// (`open`) can mean an open assumption or an open bet, told apart by the item's `kind`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum GroundingItemState {
    // A prior decision.
    Holds,
    Superseded {
        by_id: String,
        /// Title of the superseding decision, for text renderers.
        #[serde(skip_serializing_if = "Option::is_none")]
        by_label: Option<String>,
        /// When the superseding decision was recorded: the date the premise stopped standing.
        #[serde(skip_serializing_if = "Option::is_none")]
        by_recorded_at: Option<DateTime<Utc>>,
    },
    Rejected,
    Contested,
    // An observation.
    Recorded {
        #[serde(skip_serializing_if = "Option::is_none")]
        ts: Option<DateTime<Utc>>,
        /// Where it was seen (URL, file@commit, test run, measurement), as given at capture.
        #[serde(skip_serializing_if = "Option::is_none")]
        source: Option<String>,
    },
    // An assumption.
    Open,
    Supported,
    Refuted,
    // A bet. `open` / `held` / `failed` share their wire names with the assumption states.
    #[serde(rename = "open")]
    BetOpen {
        #[serde(skip_serializing_if = "Option::is_none")]
        check_by: Option<DateTime<Utc>>,
        /// The check date has passed with no evidence recorded either way.
        overdue: bool,
    },
    #[serde(rename = "held")]
    BetHeld,
    #[serde(rename = "failed")]
    BetFailed,
}

/// Whether a grounding item was named when the decision was captured or attributed afterwards.
/// Nobody can rewrite what the decider had in mind; they can only say what they attribute.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "when", rename_all = "snake_case")]
pub enum GroundingAdded {
    AtCapture,
    /// Added after capture. Absent parts were not recorded on the edge.
    Later {
        #[serde(skip_serializing_if = "Option::is_none")]
        actor_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        ts: Option<DateTime<Utc>>,
    },
}

impl GroundingItemState {
    /// The state as a phrase in a line of text — `still holds`, `SUPERSEDED 2026-09-21 by "X"`,
    /// `check by 2026-10-15, open`, `FAILED` — or `None` for an observation, which was recorded
    /// and has no later question to answer in the line itself. Capitals mark a premise that no
    /// longer stands. Deterministic; the same words in every text surface.
    pub fn describe(&self) -> Option<String> {
        Some(match self {
            Self::Holds => "still holds".to_owned(),
            Self::Superseded {
                by_id,
                by_label,
                by_recorded_at,
            } => {
                let by = by_label.as_deref().unwrap_or(by_id.as_str());
                match by_recorded_at {
                    Some(at) => format!("SUPERSEDED {} by \"{by}\"", at.format("%Y-%m-%d")),
                    None => format!("SUPERSEDED by \"{by}\""),
                }
            }
            Self::Rejected => "REJECTED".to_owned(),
            Self::Contested => "CONTESTED".to_owned(),
            Self::Recorded { .. } => return None,
            Self::Open => "open".to_owned(),
            Self::Supported => "supported".to_owned(),
            Self::Refuted => "REFUTED".to_owned(),
            Self::BetOpen { check_by, overdue } => {
                let check = match check_by {
                    Some(check_by) if *overdue => {
                        format!("check by {} (OVERDUE)", check_by.format("%Y-%m-%d"))
                    }
                    Some(check_by) => format!("check by {}", check_by.format("%Y-%m-%d")),
                    None => "no check date".to_owned(),
                };
                format!("{check}, open")
            }
            Self::BetHeld => "held".to_owned(),
            Self::BetFailed => "FAILED".to_owned(),
        })
    }
}

impl GroundingAdded {
    /// `at capture` or `attributed later by human:alex on 2026-09-22`, never blended.
    pub fn describe(&self) -> String {
        match self {
            Self::AtCapture => "at capture".to_owned(),
            Self::Later { actor_id, ts } => {
                let mut text = "attributed later".to_owned();
                if let Some(actor_id) = actor_id {
                    text.push_str(&format!(" by {actor_id}"));
                }
                if let Some(ts) = ts {
                    text.push_str(&format!(" on {}", ts.format("%Y-%m-%d")));
                }
                text
            }
        }
    }
}

/// One thing a decision rests on. `label` is the text a person reads (a decision's title, an
/// observation's content, an assumption's or bet's statement); `id` is the handle.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GroundingItem {
    pub kind: GroundingKind,
    pub id: String,
    pub label: String,
    #[serde(flatten)]
    pub state: GroundingItemState,
    pub added: GroundingAdded,
    /// A bet's "what would change our mind", in the decider's words.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub would_change_if: Option<String>,
}

/// A bet whose check date has passed with nothing recorded either way. Attention, not
/// staleness: it does not flip `held_up` (hivemind-qo11's rule).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UncheckedBet {
    pub hypothesis_id: String,
    pub check_by: DateTime<Utc>,
}

/// Everything one decision rests on, plus how many other decisions rest on it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Grounding {
    pub items: Vec<GroundingItem>,
    pub state: GroundingState,
    /// Distinct decisions that follow from this one (incoming `FOLLOWS_FROM`).
    pub dependents_count: usize,
}

// ---------------------------------------------------------------------------
// Derivations shared with the outcome, search and digest readers
// ---------------------------------------------------------------------------

/// The overall grounding of a decision from what it rests on. A decision can be both grounded
/// and carry a bet; grounded wins, and the bet is still listed.
pub fn grounding_state_of(
    premise_decisions: usize,
    evidence: usize,
    hypothesis_kinds: impl IntoIterator<Item = HypothesisKind>,
) -> GroundingState {
    let mut assumptions = 0_usize;
    let mut bets = 0_usize;
    for kind in hypothesis_kinds {
        match kind {
            HypothesisKind::Assumption => assumptions += 1,
            HypothesisKind::Bet => bets += 1,
        }
    }
    if premise_decisions + evidence + assumptions > 0 {
        GroundingState::Grounded
    } else if bets > 0 {
        GroundingState::Bet
    } else {
        GroundingState::NothingDeclared
    }
}

/// `rests on: decision x1, evidence x2, bet (check by 2026-10-15)` / `rests on: nothing
/// declared` — the one-line digest form. `bet_check_dates` has one entry per bet.
pub fn rests_on_clause(
    premise_decisions: usize,
    evidence: usize,
    assumptions: usize,
    bet_check_dates: &[Option<DateTime<Utc>>],
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if premise_decisions > 0 {
        parts.push(format!("decision x{premise_decisions}"));
    }
    if evidence > 0 {
        parts.push(format!("evidence x{evidence}"));
    }
    if assumptions > 0 {
        parts.push(format!("assumption x{assumptions}"));
    }
    for check_by in bet_check_dates {
        parts.push(match check_by {
            Some(check_by) => format!("bet (check by {})", check_by.format("%Y-%m-%d")),
            None => "bet".to_owned(),
        });
    }
    if parts.is_empty() {
        "rests on: nothing declared".to_owned()
    } else {
        format!("rests on: {}", parts.join(", "))
    }
}

/// The type-level fact a hypothesis node carries about itself.
#[derive(Clone, Debug)]
pub(crate) struct HypothesisFacts {
    pub statement: Option<String>,
    pub kind: HypothesisKind,
    pub check_by: Option<DateTime<Utc>>,
    pub would_change_if: Option<String>,
}

pub(crate) fn hypothesis_facts_from_row(row: &GraphRow) -> Result<HypothesisFacts> {
    let kind = match optional_string(row, "kind").as_deref() {
        Some("bet") => HypothesisKind::Bet,
        // `assumption`, absent (events recorded before the field existed) or unknown.
        _ => HypothesisKind::Assumption,
    };
    Ok(HypothesisFacts {
        statement: optional_string(row, "statement"),
        kind,
        check_by: optional_datetime(row, "check_by")?,
        would_change_if: optional_string(row, "would_change_if"),
    })
}

/// One hypothesis node's facts; a hypothesis that is not in the graph reads as a plain assumption.
pub(crate) fn hypothesis_facts(
    graph: &impl GraphView,
    hypothesis_id: &str,
) -> Result<HypothesisFacts> {
    match node_row(graph, NodeKind::Hypothesis, hypothesis_id)? {
        Some(row) => hypothesis_facts_from_row(&row),
        None => Ok(HypothesisFacts {
            statement: None,
            kind: HypothesisKind::Assumption,
            check_by: None,
            would_change_if: None,
        }),
    }
}

/// Prior decisions this decision follows from, distinct and sorted.
pub(crate) fn premise_decision_ids(
    graph: &impl GraphView,
    decision_id: &str,
) -> Result<Vec<String>> {
    let mut ids: Vec<String> = neighbor_pairs(
        graph,
        NodeKind::Decision,
        decision_id,
        RelationKind::FollowsFrom,
        NodeKind::Decision,
        Direction::Outgoing,
    )?
    .into_iter()
    .map(|(id, _)| id)
    .collect();
    ids.sort();
    ids.dedup();
    Ok(ids)
}

/// How many distinct decisions follow from this one.
pub fn dependents_count(graph: &impl GraphView, decision_id: &str) -> Result<usize> {
    let mut ids: Vec<String> = neighbor_pairs(
        graph,
        NodeKind::Decision,
        decision_id,
        RelationKind::FollowsFrom,
        NodeKind::Decision,
        Direction::Incoming,
    )?
    .into_iter()
    .map(|(id, _)| id)
    .collect();
    ids.sort();
    ids.dedup();
    Ok(ids.len())
}

/// Whether a prior decision still stands. `Contested` is shown, not treated as stale: the
/// disagreement is real and unresolved, not a verdict.
pub(crate) enum PremiseStanding {
    Holds,
    Superseded { by_id: String },
    Rejected,
    Contested,
}

pub(crate) fn premise_standing(
    graph: &impl GraphView,
    premise_id: &str,
) -> Result<PremiseStanding> {
    Ok(match derive_decision_status(graph, premise_id)? {
        DecisionStatus::Superseded => PremiseStanding::Superseded {
            by_id: query_superseder(graph, premise_id)?
                .map(|(by_id, _)| by_id)
                .unwrap_or_default(),
        },
        DecisionStatus::Rejected => PremiseStanding::Rejected,
        DecisionStatus::Contested => PremiseStanding::Contested,
        DecisionStatus::Proposed | DecisionStatus::Accepted => PremiseStanding::Holds,
    })
}

/// A prior decision that no longer stands, for the outcome reasons.
pub(crate) enum StalePremise {
    Superseded { decision_id: String, by_id: String },
    Rejected { decision_id: String },
}

/// What `derive_outcome` needs from grounding: which premises have gone stale and which bets are
/// overdue.
pub(crate) struct PremiseSignals {
    pub stale: Vec<StalePremise>,
    pub unchecked: Vec<UncheckedBet>,
}

pub(crate) fn premise_signals(
    graph: &impl GraphView,
    decision_id: &str,
    now: DateTime<Utc>,
) -> Result<PremiseSignals> {
    let premise_ids = premise_decision_ids(graph, decision_id)?;
    let mut stale = Vec::new();
    for premise_id in &premise_ids {
        match premise_standing(graph, premise_id)? {
            PremiseStanding::Superseded { by_id } => stale.push(StalePremise::Superseded {
                decision_id: premise_id.clone(), // ubs:ignore: owned copy for the outcome reason; premise_ids is still iterated
                by_id,
            }),
            PremiseStanding::Rejected => stale.push(StalePremise::Rejected {
                decision_id: premise_id.clone(), // ubs:ignore: owned copy for the outcome reason; premise_ids is still iterated
            }),
            PremiseStanding::Holds | PremiseStanding::Contested => {}
        }
    }

    let mut unchecked = Vec::new();
    for hypothesis_id in premised_on_hypothesis_ids(graph, decision_id)? {
        let facts = hypothesis_facts(graph, &hypothesis_id)?;
        if facts.kind != HypothesisKind::Bet {
            continue;
        }
        let Some(check_by) = facts.check_by else {
            continue;
        };
        if check_by < now
            && derive_hypothesis_status(graph, &hypothesis_id)? == HypothesisStatus::Open
        {
            unchecked.push(UncheckedBet {
                hypothesis_id,
                check_by,
            });
        }
    }

    Ok(PremiseSignals { stale, unchecked })
}

// ---------------------------------------------------------------------------
// grounding_of
// ---------------------------------------------------------------------------

/// Everything this decision rests on, with each item's state and provenance. Reads the clock
/// once for bet check dates; see `grounding_of_at` for a deterministic variant.
pub fn grounding_of(graph: &impl GraphView, decision_id: &str) -> Result<Grounding> {
    grounding_of_at(graph, decision_id, Utc::now())
}

/// `grounding_of` as of `now`. A decision that is not in the graph reads as nothing declared;
/// callers check existence first.
pub fn grounding_of_at(
    graph: &impl GraphView,
    decision_id: &str,
    now: DateTime<Utc>,
) -> Result<Grounding> {
    let proposal_event = node_row(graph, NodeKind::Decision, decision_id)?
        .and_then(|row| optional_int(&row, "event_origin"));

    let mut items: Vec<GroundingItem> = Vec::new();

    for (premise_id, edge) in grounding_edges(
        graph,
        NodeKind::Decision,
        decision_id,
        RelationKind::FollowsFrom,
        NodeKind::Decision,
    )? {
        items.push(decision_item(
            graph,
            premise_id,
            edge.added(proposal_event),
        )?);
    }

    for (evidence_id, edge) in grounding_edges(
        graph,
        NodeKind::Decision,
        decision_id,
        RelationKind::BasedOn,
        NodeKind::Evidence,
    )? {
        items.push(evidence_item(
            graph,
            evidence_id,
            edge.added(proposal_event),
        )?);
    }

    for (hypothesis_id, edge) in hypothesis_grounding_edges(graph, decision_id)? {
        items.push(hypothesis_item(
            graph,
            hypothesis_id,
            edge.added(proposal_event),
            now,
        )?);
    }

    items.sort_by(|left, right| (left.kind, &left.id).cmp(&(right.kind, &right.id)));

    let evidence = items
        .iter()
        .filter(|item| item.kind == GroundingKind::Evidence)
        .count();
    let premise_decisions = items
        .iter()
        .filter(|item| item.kind == GroundingKind::Decision)
        .count();
    let state = grounding_state_of(
        premise_decisions,
        evidence,
        items.iter().filter_map(|item| match item.kind {
            GroundingKind::Assumption => Some(HypothesisKind::Assumption),
            GroundingKind::Bet => Some(HypothesisKind::Bet),
            GroundingKind::Decision | GroundingKind::Evidence => None,
        }),
    );

    Ok(Grounding {
        items,
        state,
        dependents_count: dependents_count(graph, decision_id)?,
    })
}

/// The assumptions and bets a decision rests on, keyed by hypothesis id. An assumption or bet
/// hangs off the chosen option when there is one (named at capture) and off the decision itself
/// when grounded later; the same hypothesis reached both ways is one item, attributed to its
/// earliest edge.
fn hypothesis_grounding_edges(
    graph: &impl GraphView,
    decision_id: &str,
) -> Result<BTreeMap<String, EdgeProvenance>> {
    let mut hypothesis_edges = grounding_edges(
        graph,
        NodeKind::Decision,
        decision_id,
        RelationKind::PremisedOnDirect,
        NodeKind::Hypothesis,
    )?;
    for option_id in neighbor_ids(
        graph,
        decision_id,
        RelationKind::Chose,
        NodeKind::Option,
        "option_id",
    )? {
        for (hypothesis_id, edge) in grounding_edges(
            graph,
            NodeKind::Option,
            &option_id,
            RelationKind::PremisedOn,
            NodeKind::Hypothesis,
        )? {
            let keep_existing = hypothesis_edges
                .get(&hypothesis_id)
                .is_some_and(|existing| existing.is_not_later_than(&edge));
            if !keep_existing {
                hypothesis_edges.insert(hypothesis_id, edge);
            }
        }
    }
    Ok(hypothesis_edges)
}

/// The prior decisions a decision follows from (`FOLLOWS_FROM`) and how each was attached. Same
/// contract as `evidence_attachments`.
pub(super) fn premise_attachments(
    graph: &impl GraphView,
    decision_id: &str,
    proposal_event: Option<i64>,
) -> Result<Vec<(String, GroundingAdded)>> {
    Ok(grounding_edges(
        graph,
        NodeKind::Decision,
        decision_id,
        RelationKind::FollowsFrom,
        NodeKind::Decision,
    )?
    .into_iter()
    .map(|(premise_id, edge)| (premise_id, edge.added(proposal_event)))
    .collect())
}

/// The assumptions and bets a decision rests on and how each was attached. Same contract as
/// `evidence_attachments`.
pub(super) fn hypothesis_attachments(
    graph: &impl GraphView,
    decision_id: &str,
    proposal_event: Option<i64>,
) -> Result<Vec<(String, GroundingAdded)>> {
    Ok(hypothesis_grounding_edges(graph, decision_id)?
        .into_iter()
        .map(|(hypothesis_id, edge)| (hypothesis_id, edge.added(proposal_event)))
        .collect())
}

/// The evidence a decision cites (`BASED_ON`) and how each item was attached: named at capture
/// or attributed afterwards. Distinct ids, sorted. `proposal_event` is the decision's own
/// `event_origin`, which the caller has already read.
pub(super) fn evidence_attachments(
    graph: &impl GraphView,
    decision_id: &str,
    proposal_event: Option<i64>,
) -> Result<Vec<(String, GroundingAdded)>> {
    Ok(grounding_edges(
        graph,
        NodeKind::Decision,
        decision_id,
        RelationKind::BasedOn,
        NodeKind::Evidence,
    )?
    .into_iter()
    .map(|(evidence_id, edge)| (evidence_id, edge.added(proposal_event)))
    .collect())
}

fn decision_item(
    graph: &impl GraphView,
    premise_id: String,
    added: GroundingAdded,
) -> Result<GroundingItem> {
    let label = node_row(graph, NodeKind::Decision, &premise_id)?
        .and_then(|row| optional_string(&row, "title"))
        .unwrap_or_else(|| premise_id.clone()); // ubs:ignore: fall back to the id when the premise has no title; the id is still needed below
    let state = match premise_standing(graph, &premise_id)? {
        PremiseStanding::Holds => GroundingItemState::Holds,
        PremiseStanding::Rejected => GroundingItemState::Rejected,
        PremiseStanding::Contested => GroundingItemState::Contested,
        PremiseStanding::Superseded { by_id } => {
            let by_row = if by_id.is_empty() {
                None
            } else {
                node_row(graph, NodeKind::Decision, &by_id)?
            };
            GroundingItemState::Superseded {
                by_label: by_row
                    .as_ref()
                    .and_then(|row| optional_string(row, "title")),
                by_recorded_at: by_row
                    .as_ref()
                    .map(|row| optional_datetime(row, "occurred_at"))
                    .transpose()?
                    .flatten(),
                by_id,
            }
        }
    };
    Ok(GroundingItem {
        kind: GroundingKind::Decision,
        id: premise_id,
        label,
        state,
        added,
        would_change_if: None,
    })
}

fn evidence_item(
    graph: &impl GraphView,
    evidence_id: String,
    added: GroundingAdded,
) -> Result<GroundingItem> {
    let row = node_row(graph, NodeKind::Evidence, &evidence_id)?;
    let label = row
        .as_ref()
        .and_then(|row| optional_string(row, "content"))
        .unwrap_or_else(|| evidence_id.clone()); // ubs:ignore: fall back to the id when the node has no content; the id is still needed below
    let state = GroundingItemState::Recorded {
        ts: row
            .as_ref()
            .map(|row| optional_datetime(row, "recorded_at"))
            .transpose()?
            .flatten(),
        source: row
            .as_ref()
            .and_then(|row| optional_string(row, "evidence_source")),
    };
    Ok(GroundingItem {
        kind: GroundingKind::Evidence,
        id: evidence_id,
        label,
        state,
        added,
        would_change_if: None,
    })
}

fn hypothesis_item(
    graph: &impl GraphView,
    hypothesis_id: String,
    added: GroundingAdded,
    now: DateTime<Utc>,
) -> Result<GroundingItem> {
    let facts = hypothesis_facts(graph, &hypothesis_id)?;
    let status = derive_hypothesis_status(graph, &hypothesis_id)?;
    let (kind, state) = match facts.kind {
        HypothesisKind::Assumption => (
            GroundingKind::Assumption,
            match status {
                HypothesisStatus::Open => GroundingItemState::Open,
                HypothesisStatus::Supported => GroundingItemState::Supported,
                HypothesisStatus::Refuted => GroundingItemState::Refuted,
            },
        ),
        HypothesisKind::Bet => (
            GroundingKind::Bet,
            match status {
                HypothesisStatus::Open => GroundingItemState::BetOpen {
                    check_by: facts.check_by,
                    overdue: facts.check_by.is_some_and(|check_by| check_by < now),
                },
                HypothesisStatus::Supported => GroundingItemState::BetHeld,
                HypothesisStatus::Refuted => GroundingItemState::BetFailed,
            },
        ),
    };
    let label = facts.statement.unwrap_or_else(|| hypothesis_id.clone()); // ubs:ignore: fall back to the id when the node has no statement; the id is still needed below
    Ok(GroundingItem {
        kind,
        id: hypothesis_id,
        label,
        state,
        added,
        would_change_if: facts.would_change_if,
    })
}

// ---------------------------------------------------------------------------
// Edge provenance
// ---------------------------------------------------------------------------

/// What the projector recorded on one grounding edge (`projector::grounding_edge_properties`).
struct EdgeProvenance {
    event_origin: Option<i64>,
    causation_event_id: Option<i64>,
    added_by: Option<String>,
    added_at: Option<DateTime<Utc>>,
}

impl EdgeProvenance {
    /// At capture when the edge was caused by the decision's own proposal event (the fan-out
    /// relation events and the proposal's own payload edges both carry it); otherwise the actor
    /// and time that attributed it.
    fn added(&self, proposal_event: Option<i64>) -> GroundingAdded {
        if proposal_event.is_some() && self.causation_event_id == proposal_event {
            GroundingAdded::AtCapture
        } else {
            GroundingAdded::Later {
                actor_id: self.added_by.clone(), // ubs:ignore: owned copy for the item; the edge map is dropped after this call
                ts: self.added_at,
            }
        }
    }

    fn is_not_later_than(&self, other: &Self) -> bool {
        match (self.event_origin, other.event_origin) {
            (Some(mine), Some(theirs)) => mine <= theirs,
            _ => true,
        }
    }
}

/// `root`'s outgoing edges of one grounding relation, keyed by the other end. When the same pair
/// was asserted more than once, the earliest assertion is the one attributed (memory returns one
/// row per assertion, oldest first; Postgres keeps the first write).
fn grounding_edges(
    graph: &impl GraphView,
    root_kind: NodeKind,
    root_id: &str,
    relation: RelationKind,
    other_kind: NodeKind,
) -> Result<BTreeMap<String, EdgeProvenance>> {
    let root_table = root_kind.table_name();
    let relation_table = relation.table_name();
    let other_table = other_kind.table_name();
    let rows = graph.query(
        &format!(
            "MATCH (a:`{root_table}` {{id: $id}})-[r:`{relation_table}`]->(b:`{other_table}`) RETURN b.id AS id, r.event_origin AS event_origin, r.causation_event_id AS causation_event_id, r.added_by AS added_by, r.added_at AS added_at ORDER BY b.id;"
        ),
        &GraphParams::from([("id".to_owned(), GraphValue::String(root_id.to_owned()))]),
    )?;
    let mut edges = BTreeMap::new();
    for row in rows {
        let id = required_string(&row, "id")?;
        if edges.contains_key(&id) {
            continue;
        }
        edges.insert(
            id,
            EdgeProvenance {
                event_origin: optional_int(&row, "event_origin"),
                causation_event_id: optional_int(&row, "causation_event_id"),
                added_by: optional_string(&row, "added_by"),
                added_at: optional_datetime(&row, "added_at")?,
            },
        );
    }
    Ok(edges)
}

#[cfg(test)]
mod tests;
