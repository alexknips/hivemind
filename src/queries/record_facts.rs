//! What one decision's record states, as plain facts: the input the quality profile reads.
//!
//! Pure Layer-2 reads: anchored lookups on the one decision and its direct options and
//! evidence (never a scan of the graph), no interpretation, no ranking, no LLM. Nothing here
//! says whether the record is good. That judgement is the profile's, in Layer 3, and this module
//! knows nothing of it.
//!
//! Every fact is read through `node_row` and the anchored neighbour reads the other queries use,
//! so memory, Postgres and Kuzu agree on it. A decision node need not come from
//! `decision.proposed` (a bare stub, a classified capture): every field tolerates absence.

use crate::projector::{GraphView, NodeKind, RelationKind};
use crate::Result;

use super::grounding::{evidence_attachments, GroundingAdded};
use super::shared::{neighbor_ids, node_row, optional_int, optional_string};

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

/// What a decision's record states, in a fixed order (options and evidence sorted by id).
#[derive(Clone, Debug, PartialEq)]
pub struct RecordFacts {
    pub decision_id: String,
    /// Ledger offset of the event that recorded the decision.
    pub event_origin: Option<i64>,
    /// The question the decision answers, when one was recorded.
    pub question: Option<String>,
    pub rationale: Option<String>,
    pub chosen_option_id: Option<String>,
    /// Every option the decision has (`HAS_OPTION`), the chosen one included.
    pub options: Vec<OptionFact>,
    pub evidence: Vec<EvidenceFact>,
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

    Ok(Some(RecordFacts {
        decision_id: decision_id.to_owned(),
        event_origin,
        question: optional_string(&row, "question"),
        rationale: optional_string(&row, "rationale"),
        chosen_option_id,
        options,
        evidence,
    }))
}
