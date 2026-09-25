//! What one decision's record states, as plain facts: the input the quality profile reads.
//!
//! Pure Layer-2 reads: anchored lookups on the one decision and its direct options, evidence,
//! prior decisions and assumptions or bets (never a scan of the graph), no interpretation, no
//! ranking, no LLM. Nothing here says whether the record is good. That judgement is the
//! profile's, in Layer 3, and this module knows nothing of it.
//!
//! Every fact is read through `node_row` and the anchored neighbour reads the other queries use,
//! so memory, Postgres and Kuzu agree on it. A decision node need not come from
//! `decision.proposed` (a bare stub, a classified capture): every field tolerates absence.

use chrono::{DateTime, Utc};

use crate::events::HypothesisKind;
use crate::projector::{GraphView, NodeKind, RelationKind};
use crate::Result;

use super::grounding::{
    evidence_attachments, hypothesis_attachments, hypothesis_facts_from_row, premise_attachments,
    GroundingAdded,
};
use super::shared::{
    neighbor_ids, neighbor_pairs, node_row, optional_datetime, optional_int, optional_string,
    query_superseder, Direction,
};

/// One option recorded on a decision, as its node states it.
#[derive(Clone, Debug, PartialEq)]
pub struct OptionFact {
    pub id: String,
    /// The label recorded with the option; absent on events from before labels existed.
    pub label: Option<String>,
    /// The description recorded with the option, verbatim. It may be text a capture surface
    /// filled in when the author gave none: this fact does not say which.
    pub description: Option<String>,
}

/// One evidence item a decision cites, with how it came to be cited.
#[derive(Clone, Debug, PartialEq)]
pub struct EvidenceFact {
    pub id: String,
    /// Ledger offset of the event that recorded the evidence.
    pub event_origin: Option<i64>,
    /// Where the observation was made (URL, file@commit, test run, measurement), as given.
    pub source: Option<String>,
    /// Named in the decision's own proposal, or attributed afterwards.
    pub added: GroundingAdded,
}

/// The decision that replaced a prior decision.
#[derive(Clone, Debug, PartialEq)]
pub struct SupersessionFact {
    pub by_id: String,
    /// Ledger offset of the event that recorded the supersession.
    pub event_origin: Option<i64>,
}

/// One prior decision this decision follows from (`FOLLOWS_FROM`), with how it came to be cited.
#[derive(Clone, Debug, PartialEq)]
pub struct PremiseFact {
    pub id: String,
    /// Ledger offset of the event that recorded the prior decision.
    pub event_origin: Option<i64>,
    /// When the prior decision was made, as its own record states.
    pub occurred_at: Option<DateTime<Utc>>,
    /// Named in the decision's own proposal, or attributed afterwards.
    pub added: GroundingAdded,
    /// Set when the prior decision has since been replaced. Says nothing about whether that
    /// happened before or after this decision: the two offsets do.
    pub superseded: Option<SupersessionFact>,
}

/// Evidence that refutes a hypothesis, with when the evidence was recorded.
#[derive(Clone, Debug, PartialEq)]
pub struct RefutationFact {
    pub evidence_id: String,
    /// Ledger offset of the event that recorded the evidence.
    pub evidence_origin: Option<i64>,
}

/// One assumption or declared bet a decision rests on, with how it came to be cited.
#[derive(Clone, Debug, PartialEq)]
pub struct HypothesisFact {
    pub id: String,
    pub kind: HypothesisKind,
    /// Ledger offset of the event that recorded the hypothesis.
    pub event_origin: Option<i64>,
    /// Named in the decision's own proposal, or attributed afterwards.
    pub added: GroundingAdded,
    /// Every evidence item that refutes it, sorted by id.
    pub refuted_by: Vec<RefutationFact>,
}

/// What a decision's record states, in a fixed order (every list sorted by id).
#[derive(Clone, Debug, PartialEq)]
pub struct RecordFacts {
    pub decision_id: String,
    /// Ledger offset of the event that recorded the decision.
    pub event_origin: Option<i64>,
    /// When the decision was made, as its own record states.
    pub occurred_at: Option<DateTime<Utc>>,
    /// The question the decision answers, when one was recorded.
    pub question: Option<String>,
    pub rationale: Option<String>,
    /// The confidence the decider declared at capture, verbatim.
    pub expressed_confidence: Option<String>,
    pub chosen_option_id: Option<String>,
    /// Every option the decision has (`HAS_OPTION`), the chosen one included.
    pub options: Vec<OptionFact>,
    pub evidence: Vec<EvidenceFact>,
    pub premises: Vec<PremiseFact>,
    pub hypotheses: Vec<HypothesisFact>,
}

/// The facts of one decision's record, or `None` when the decision does not exist.
pub fn get_record_facts(graph: &impl GraphView, decision_id: &str) -> Result<Option<RecordFacts>> {
    let Some(row) = node_row(graph, NodeKind::Decision, decision_id)? else {
        return Ok(None);
    };
    let event_origin = optional_int(&row, "event_origin");

    let mut options = Vec::new();
    for option_id in neighbor_ids(
        graph,
        decision_id,
        RelationKind::HasOption,
        NodeKind::Option,
        "option_id",
    )? {
        let option_row = node_row(graph, NodeKind::Option, &option_id)?;
        options.push(OptionFact {
            label: option_row
                .as_ref()
                .and_then(|row| optional_string(row, "label")),
            description: option_row
                .as_ref()
                .and_then(|row| optional_string(row, "description")),
            id: option_id,
        });
    }

    let chosen_option_id = neighbor_ids(
        graph,
        decision_id,
        RelationKind::Chose,
        NodeKind::Option,
        "option_id",
    )?
    .into_iter()
    .next();

    let mut evidence = Vec::new();
    for (evidence_id, added) in evidence_attachments(graph, decision_id, event_origin)? {
        let evidence_row = node_row(graph, NodeKind::Evidence, &evidence_id)?;
        evidence.push(EvidenceFact {
            event_origin: evidence_row
                .as_ref()
                .and_then(|row| optional_int(row, "event_origin")),
            source: evidence_row
                .as_ref()
                .and_then(|row| optional_string(row, "evidence_source")),
            added,
            id: evidence_id,
        });
    }

    let mut premises = Vec::new();
    for (premise_id, added) in premise_attachments(graph, decision_id, event_origin)? {
        let premise_row = node_row(graph, NodeKind::Decision, &premise_id)?;
        let superseded =
            query_superseder(graph, &premise_id)?.map(|(by_id, event_origin)| SupersessionFact {
                by_id,
                event_origin,
            });
        premises.push(PremiseFact {
            event_origin: premise_row
                .as_ref()
                .and_then(|row| optional_int(row, "event_origin")),
            occurred_at: premise_row
                .as_ref()
                .map(|row| optional_datetime(row, "occurred_at"))
                .transpose()?
                .flatten(),
            added,
            superseded,
            id: premise_id,
        });
    }

    let mut hypotheses = Vec::new();
    for (hypothesis_id, added) in hypothesis_attachments(graph, decision_id, event_origin)? {
        let hypothesis_row = node_row(graph, NodeKind::Hypothesis, &hypothesis_id)?;
        // A hypothesis that is not in the graph reads as a plain assumption, like everywhere else.
        let kind = match &hypothesis_row {
            Some(row) => hypothesis_facts_from_row(row)?.kind,
            None => HypothesisKind::Assumption,
        };
        let mut refuted_by = Vec::new();
        for (evidence_id, _link_origin) in neighbor_pairs(
            graph,
            NodeKind::Hypothesis,
            &hypothesis_id,
            RelationKind::Refutes,
            NodeKind::Evidence,
            Direction::Incoming,
        )? {
            let evidence_row = node_row(graph, NodeKind::Evidence, &evidence_id)?;
            refuted_by.push(RefutationFact {
                evidence_origin: evidence_row
                    .as_ref()
                    .and_then(|row| optional_int(row, "event_origin")),
                evidence_id,
            });
        }
        hypotheses.push(HypothesisFact {
            event_origin: hypothesis_row
                .as_ref()
                .and_then(|row| optional_int(row, "event_origin")),
            kind,
            added,
            refuted_by,
            id: hypothesis_id,
        });
    }

    Ok(Some(RecordFacts {
        decision_id: decision_id.to_owned(),
        event_origin,
        occurred_at: optional_datetime(&row, "occurred_at")?,
        question: optional_string(&row, "question"),
        rationale: optional_string(&row, "rationale"),
        expressed_confidence: optional_string(&row, "expressed_confidence"),
        chosen_option_id,
        options,
        evidence,
        premises,
        hypotheses,
    }))
}
