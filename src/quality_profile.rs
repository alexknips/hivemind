//! Decision quality profile: the seven dimensions of `docs/DECISION_SCORING.md` for every
//! decision, however it was captured, each from a deterministic floor over stated facts.
//!
//! A floor says what the record states, never that it is sound. Each dimension is either
//! *assessed* (an ordinal level, the reasons behind it and the ids of the nodes they rest on) or
//! *not assessed* (and why). There is no composite number, no tier and no grade: a dimension with
//! no basis reports that it has none rather than a guess.
//!
//! # Rules every floor follows
//! - **Ex ante.** Something counts toward a floor only if it was recorded before the decision or
//!   attached at capture. Evidence recorded afterwards is shown as `later` and never raises a
//!   level; a record that pre-dates the decision counts even if it was linked afterwards.
//! - **Reasoning and Framing stop at `partial`** without a model: they can say the rationale or
//!   the question is on record, not that it is sound or the right one.
//! - **Reasons and ids are deterministic:** the same graph gives the same profile, byte for byte.
//!
//! # Placement
//! Layer 3 (`ARCHITECTURE.md` → Layer Boundary). It reads the graph through `queries` and nothing
//! in `queries/` or `commands/` imports this module (`tests::queries_and_commands_never_import_the_profile`
//! holds that line). It reads one decision and its direct options and evidence with anchored
//! lookups: no scan, no model, no network, no write.
//!
//! [`FLOOR_VERSION`] moves whenever a rule below changes what level a record gets.

use std::collections::BTreeSet;

use serde::Serialize;

use crate::projector::GraphView;
use crate::queries::{get_record_facts, EvidenceFact, GroundingAdded, OptionFact, RecordFacts};
use crate::Result;

/// The version of the floor rules in this module.
pub const FLOOR_VERSION: u32 = 1;

/// The seven dimensions, in the order `docs/DECISION_SCORING.md` lists them.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Dimension {
    Framing,
    Alternatives,
    Information,
    Reasoning,
    ValuesTradeoffs,
    BiasExposure,
    Calibration,
}

impl Dimension {
    pub const ALL: [Self; 7] = [
        Self::Framing,
        Self::Alternatives,
        Self::Information,
        Self::Reasoning,
        Self::ValuesTradeoffs,
        Self::BiasExposure,
        Self::Calibration,
    ];
}

/// How much of a dimension the record supports. Ordinal, from published rules; not a fraction.
/// `None` means the record states nothing toward the dimension, not that the decision was bad.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    None,
    Partial,
    Solid,
}

/// Which rule produced a reason, so a consumer can tell them apart without reading the text.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasonKind {
    QuestionRecorded,
    NoQuestionRecorded,
    NoAlternativeRecorded,
    AlternativesRecorded,
    AlternativesDescribed,
    AlternativesUndescribed,
    NoEvidenceLinked,
    EvidenceCounted,
    EvidenceSourceStated,
    EvidenceSourceMissing,
    EvidenceLater,
    RationaleStated,
    NoRationale,
    /// The dimension needs a judgement, so the floor stops at `partial`.
    NotJudgedWithoutModel,
}

/// One reason behind a level: the rule that fired, in words, and the nodes it rests on.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Reason {
    pub kind: ReasonKind,
    pub text: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub node_ids: Vec<String>,
}

/// One dimension's answer. A `NotAssessed` entry has no level, by construction.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Assessment {
    Assessed {
        level: Level,
        reasons: Vec<Reason>,
        /// Every node the reasons rest on, sorted and distinct.
        node_ids: Vec<String>,
    },
    NotAssessed {
        why: String,
    },
}

impl Assessment {
    fn assessed(level: Level, reasons: Vec<Reason>) -> Self {
        let node_ids = reasons
            .iter()
            .flat_map(|reason| reason.node_ids.iter())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .cloned()
            .collect();
        Self::Assessed {
            level,
            reasons,
            node_ids,
        }
    }

    fn not_assessed(why: &str) -> Self {
        Self::NotAssessed {
            why: why.to_owned(),
        }
    }

    /// The level, or `None` for a dimension that was not assessed.
    pub fn level(&self) -> Option<Level> {
        match self {
            Self::Assessed { level, .. } => Some(*level),
            Self::NotAssessed { .. } => None,
        }
    }
}

/// The seven dimensions for one decision, each standing alone.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct QualityProfile {
    pub decision_id: String,
    /// [`FLOOR_VERSION`] at the time the profile was computed.
    pub floor_version: u32,
    pub framing: Assessment,
    pub alternatives: Assessment,
    pub information: Assessment,
    pub reasoning: Assessment,
    pub values_tradeoffs: Assessment,
    pub bias_exposure: Assessment,
    pub calibration: Assessment,
}

impl QualityProfile {
    pub fn assessment(&self, dimension: Dimension) -> &Assessment {
        match dimension {
            Dimension::Framing => &self.framing,
            Dimension::Alternatives => &self.alternatives,
            Dimension::Information => &self.information,
            Dimension::Reasoning => &self.reasoning,
            Dimension::ValuesTradeoffs => &self.values_tradeoffs,
            Dimension::BiasExposure => &self.bias_exposure,
            Dimension::Calibration => &self.calibration,
        }
    }

    /// All seven dimensions in [`Dimension::ALL`] order.
    pub fn iter(&self) -> impl Iterator<Item = (Dimension, &Assessment)> {
        Dimension::ALL
            .into_iter()
            .map(|dimension| (dimension, self.assessment(dimension)))
    }
}

const VALUES_TRADEOFFS_WHY: &str = "Judged only: nothing recorded can stand in for a judgement of whether the values and tradeoffs were made explicit and weighed, and no model assessment is attached.";
const BIAS_EXPOSURE_WHY: &str = "No floor in this version: it would report whether a counter-option or counter-evidence was recorded and how old the premises were, and that is not computed yet. Nothing is inferred in its place.";
const CALIBRATION_WHY: &str = "No floor in this version: it would compare the confidence the decider declared at capture with what the decision rests on (evidence, a prior decision, a declared bet), and that is not computed yet. Nothing is inferred in its place.";

/// The profile of one decision, or `None` when the decision does not exist. Read-only.
pub fn quality_profile_of(
    graph: &impl GraphView,
    decision_id: &str,
) -> Result<Option<QualityProfile>> {
    Ok(get_record_facts(graph, decision_id)?.map(|facts| profile_from_record(&facts)))
}

/// The profile a record's facts support. Pure: the same facts give the same profile.
pub fn profile_from_record(facts: &RecordFacts) -> QualityProfile {
    QualityProfile {
        decision_id: facts.decision_id.clone(),
        floor_version: FLOOR_VERSION,
        framing: framing(facts),
        alternatives: alternatives(facts),
        information: information(facts),
        reasoning: reasoning(facts),
        values_tradeoffs: Assessment::not_assessed(VALUES_TRADEOFFS_WHY),
        bias_exposure: Assessment::not_assessed(BIAS_EXPOSURE_WHY),
        calibration: Assessment::not_assessed(CALIBRATION_WHY),
    }
}

fn reason(kind: ReasonKind, text: String, node_ids: Vec<String>) -> Reason {
    Reason {
        kind,
        text,
        node_ids,
    }
}

fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// `Some(text)` when there is text beyond whitespace.
fn stated(text: Option<&str>) -> Option<&str> {
    text.map(str::trim).filter(|text| !text.is_empty())
}

// ---------------------------------------------------------------------------
// Framing
// ---------------------------------------------------------------------------

/// `none` when no question is recorded; `partial` when one is. Never higher without a model:
/// whether it is the right question is a judgement.
fn framing(facts: &RecordFacts) -> Assessment {
    if stated(facts.question.as_deref()).is_none() {
        return Assessment::assessed(
            Level::None,
            vec![reason(
                ReasonKind::NoQuestionRecorded,
                "no question is recorded: the record does not say what question this decision answers".to_owned(),
                Vec::new(),
            )],
        );
    }
    Assessment::assessed(
        Level::Partial,
        vec![
            reason(
                ReasonKind::QuestionRecorded,
                "the question this decision answers is recorded (stated, not judged well framed)"
                    .to_owned(),
                vec![facts.decision_id.clone()],
            ),
            reason(
                ReasonKind::NotJudgedWithoutModel,
                "whether it is the right question is judged, not derived: no higher than partial without a model assessment".to_owned(),
                Vec::new(),
            ),
        ],
    )
}

// ---------------------------------------------------------------------------
// Alternatives
// ---------------------------------------------------------------------------

/// Text a capture surface fills in when the author gave an option no description, keyed by the
/// surface that writes it. Each is `<prefix><label>'`. These are the literals those surfaces
/// write today (`mcp::args::default_option_description`, `cli::run::record_cli_options`,
/// `Commands::supersede`, `api::handlers` capture); the rule is documented in
/// `docs/DECISION_SCORING.md`.
const GENERATED_QUOTED_PREFIXES: [&str; 4] = [
    "Option generated from MCP value '",
    "Option generated from CLI value '",
    "Option generated from supersede value '",
    "Option '",
];
const GENERATED_SLACK_PREFIX: &str = "Slack option '";
const GENERATED_SLACK_INFIX: &str = "' captured from ";
const GENERATED_IMPORT_PREFIX: &str = "Option imported from document block ";

/// Whether `description` is only the option's own label or text a capture surface generated for
/// it. Such text carries nothing the author wrote, so it does not count as a description.
fn is_generated_description(label: &str, description: &str) -> bool {
    if description.eq_ignore_ascii_case(label) || description.starts_with(GENERATED_IMPORT_PREFIX) {
        return true;
    }
    if GENERATED_QUOTED_PREFIXES.iter().any(|prefix| {
        description
            .strip_prefix(prefix)
            .and_then(|rest| rest.strip_prefix(label))
            == Some("'")
    }) {
        return true;
    }
    description
        .strip_prefix(GENERATED_SLACK_PREFIX)
        .and_then(|rest| rest.strip_prefix(label))
        .is_some_and(|rest| rest.starts_with(GENERATED_SLACK_INFIX))
}

fn has_own_description(option: &OptionFact) -> bool {
    let Some(description) = stated(option.description.as_deref()) else {
        return false;
    };
    !is_generated_description(option.label.as_deref().map_or("", str::trim), description)
}

fn option_ids(options: &[&OptionFact]) -> Vec<String> {
    let mut ids: Vec<String> = options.iter().map(|option| option.id.clone()).collect();
    ids.sort();
    ids
}

/// The alternatives are the options other than the chosen one (all of them while none is
/// chosen). `none` when nothing was set against the option taken, i.e. fewer than two options are
/// on record; `partial` when some alternative carries no description of its own; `solid` when
/// every alternative does.
fn alternatives(facts: &RecordFacts) -> Assessment {
    let chosen = facts.chosen_option_id.as_deref();
    let others: Vec<&OptionFact> = facts
        .options
        .iter()
        .filter(|option| Some(option.id.as_str()) != chosen)
        .collect();
    let considered = others.len() + usize::from(chosen.is_some());

    if considered < 2 {
        let text = if considered == 0 {
            "no option is recorded"
        } else {
            "only one option is recorded: nothing was set against it"
        };
        return Assessment::assessed(
            Level::None,
            vec![reason(
                ReasonKind::NoAlternativeRecorded,
                text.to_owned(),
                Vec::new(),
            )],
        );
    }

    let undescribed: Vec<&OptionFact> = others
        .iter()
        .copied()
        .filter(|option| !has_own_description(option))
        .collect();
    let besides = if chosen.is_some() {
        "besides the chosen option"
    } else {
        "with no option chosen"
    };
    let mut reasons = vec![reason(
        ReasonKind::AlternativesRecorded,
        format!("{} recorded {besides}", plural(others.len(), "alternative")),
        option_ids(&others),
    )];
    let level = if undescribed.is_empty() {
        reasons.push(reason(
            ReasonKind::AlternativesDescribed,
            "every alternative carries a description of its own".to_owned(),
            option_ids(&others),
        ));
        Level::Solid
    } else {
        reasons.push(reason(
            ReasonKind::AlternativesUndescribed,
            format!(
                "{} without a description of its own (text a capture surface fills in does not count)",
                plural(undescribed.len(), "alternative"),
            ),
            option_ids(&undescribed),
        ));
        Level::Partial
    };
    Assessment::assessed(level, reasons)
}

// ---------------------------------------------------------------------------
// Information
// ---------------------------------------------------------------------------

/// Evidence counts when it was attached at capture or recorded before the decision (even if it
/// was linked afterwards). Anything else is `later`.
fn is_ex_ante(evidence: &EvidenceFact, decision_origin: Option<i64>) -> bool {
    matches!(evidence.added, GroundingAdded::AtCapture)
        || matches!(
            (evidence.event_origin, decision_origin),
            (Some(recorded), Some(decided)) if recorded < decided
        )
}

fn evidence_ids(evidence: &[&EvidenceFact]) -> Vec<String> {
    let mut ids: Vec<String> = evidence.iter().map(|item| item.id.clone()).collect();
    ids.sort();
    ids
}

/// From the evidence linked to the decision. `none` when none counts; `partial` when some does
/// but none says where it was observed; `solid` when at least one counted item does, so it can be
/// checked again.
fn information(facts: &RecordFacts) -> Assessment {
    let (counted, later): (Vec<&EvidenceFact>, Vec<&EvidenceFact>) = facts
        .evidence
        .iter()
        .partition(|item| is_ex_ante(item, facts.event_origin));
    let sourced: Vec<&EvidenceFact> = counted
        .iter()
        .copied()
        .filter(|item| stated(item.source.as_deref()).is_some())
        .collect();

    let mut reasons = Vec::new();
    let level = if counted.is_empty() {
        let text = if later.is_empty() {
            "no evidence is linked"
        } else {
            "no evidence is linked that pre-dates the decision"
        };
        reasons.push(reason(
            ReasonKind::NoEvidenceLinked,
            text.to_owned(),
            Vec::new(),
        ));
        Level::None
    } else {
        reasons.push(reason(
            ReasonKind::EvidenceCounted,
            format!(
                "{} counted: recorded before the decision or attached at capture",
                plural(counted.len(), "evidence item"),
            ),
            evidence_ids(&counted),
        ));
        if sourced.is_empty() {
            reasons.push(reason(
                ReasonKind::EvidenceSourceMissing,
                "where it was observed is stated for none".to_owned(),
                Vec::new(),
            ));
            Level::Partial
        } else {
            reasons.push(reason(
                ReasonKind::EvidenceSourceStated,
                format!(
                    "where it was observed is stated for {} of {}",
                    sourced.len(),
                    counted.len()
                ),
                evidence_ids(&sourced),
            ));
            Level::Solid
        }
    };
    if !later.is_empty() {
        reasons.push(reason(
            ReasonKind::EvidenceLater,
            format!(
                "{} attached after the decision was captured and not recorded before it (later): shown, never counted",
                plural(later.len(), "evidence item"),
            ),
            evidence_ids(&later),
        ));
    }
    Assessment::assessed(level, reasons)
}

// ---------------------------------------------------------------------------
// Reasoning
// ---------------------------------------------------------------------------

/// `none` when no rationale is recorded; `partial` when one is. Never higher without a model:
/// whether the inference from information to choice is sound is a judgement.
fn reasoning(facts: &RecordFacts) -> Assessment {
    if stated(facts.rationale.as_deref()).is_none() {
        return Assessment::assessed(
            Level::None,
            vec![reason(
                ReasonKind::NoRationale,
                "no rationale is recorded".to_owned(),
                Vec::new(),
            )],
        );
    }
    Assessment::assessed(
        Level::Partial,
        vec![
            reason(
                ReasonKind::RationaleStated,
                "a rationale is recorded (stated, not judged sound)".to_owned(),
                vec![facts.decision_id.clone()],
            ),
            reason(
                ReasonKind::NotJudgedWithoutModel,
                "whether the inference is sound is judged, not derived: no higher than partial without a model assessment".to_owned(),
                Vec::new(),
            ),
        ],
    )
}

#[cfg(test)]
pub(crate) mod tests;
