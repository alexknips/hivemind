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
//!   attached at capture. Evidence, a prior decision, an assumption or a bet recorded afterwards
//!   is shown as `later` and never raises a level; a record that pre-dates the decision counts
//!   even if it was linked afterwards.
//! - **Judged dimensions stop at `partial`** without a model: Framing, Reasoning, Bias exposure
//!   and Calibration can say what is on record, not that it is sound, right or well matched.
//! - **What happened next is not quality.** A prior decision that was superseded afterwards is
//!   shown as a fact ("since superseded (later)") and changes no level: that is outcome, which
//!   has its own view.
//! - **A mismatch is attention, not a deduction.** High declared confidence over a bet, or over
//!   nothing declared, adds an [`Attention`] line; the level does not move with the confidence.
//! - **Reasons and ids are deterministic:** the same graph gives the same profile, byte for byte.
//!
//! # Placement
//! Layer 3 (`ARCHITECTURE.md` → Layer Boundary). It reads the graph through `queries` and nothing
//! in `queries/` or `commands/` imports this module (`tests::queries_and_commands_never_import_the_profile`
//! holds that line). It reads one decision and its direct options, evidence, prior decisions and
//! assumptions or bets with anchored lookups: no scan, no model, no network, no write.
//!
//! [`FLOOR_VERSION`] moves whenever a rule below changes what level a record gets.
//!
//! # Attention findings
//! The profile answers how much of each dimension one decision's record supports. What deserves
//! a look across the graph is a separate, derived list: bets past their check date, decisions
//! whose premise changed, evidence nobody has re-checked. That is [`findings`], and it is the
//! roll-up in place of a grade.
//!
//! # Failure modes
//! The analysis of which conditions go with decisions that did not hold up takes the seven
//! dimensions as conditions ([`failure_modes`]): for each dimension, how the decisions at each
//! level fared. A level sorts decisions into groups; it is never itself a failure.
//!
//! # What people and agents see
//! [`report`] is the one core behind `score_decision`, `scan_decision_quality` and
//! `get_suggestions` on the stdio MCP server, the HTTP MCP endpoint and the CLI: the profile with
//! its provenance line, and a page of findings each with the dimensions it bears on (for
//! `get_suggestions`, without the ones already acknowledged).

use std::collections::BTreeSet;

use serde::Serialize;

use crate::events::HypothesisKind;
use crate::projector::GraphView;
use crate::queries::{
    get_record_facts, EvidenceFact, GroundingAdded, HypothesisFact, OptionFact, PremiseFact,
    RecordFacts,
};
use crate::Result;

pub mod failure_modes;
pub mod findings;
pub mod report;

pub use failure_modes::{analyze_failure_modes, profile_conditions};
pub use findings::{
    attention_findings, attention_findings_at, AttentionConfig, AttentionFinding, AttentionPage,
    AttentionRequest, FindingKind, DEFAULT_EVIDENCE_WINDOW_DAYS,
};
pub use report::{
    acknowledged_finding_ids, get_suggestions, get_suggestions_at, parse_kinds,
    scan_decision_quality, scan_decision_quality_at, score_decision, DimensionLine, Provenance,
    ScanFinding, ScanReport, ScanRequest, ScoreReport, SuggestionsRequest, NOT_REVIEWED_BY_A_HUMAN,
    SCAN_DEFAULT_LIMIT,
};

/// The version of the floor rules in this module.
///
/// 2: prior decisions, assumptions and declared bets count toward Information and prior decisions
/// toward Reasoning; Calibration and Bias exposure have floors.
pub const FLOOR_VERSION: u32 = 2;

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

    /// The wire name (`bias_exposure`), the one serialization gives.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Framing => "framing",
            Self::Alternatives => "alternatives",
            Self::Information => "information",
            Self::Reasoning => "reasoning",
            Self::ValuesTradeoffs => "values_tradeoffs",
            Self::BiasExposure => "bias_exposure",
            Self::Calibration => "calibration",
        }
    }
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

impl Level {
    /// The wire name (`partial`), the one serialization gives.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Partial => "partial",
            Self::Solid => "solid",
        }
    }
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
    /// No evidence, prior decision, assumption or bet counts.
    NothingRestedOn,
    EvidenceCounted,
    EvidenceSourceStated,
    EvidenceSourceMissing,
    EvidenceLater,
    PremiseCounted,
    /// A counted prior decision has since been superseded: a fact, never a change of level.
    PremiseSuperseded,
    PremiseLater,
    AssumptionCounted,
    AssumptionLater,
    BetCounted,
    BetLater,
    RationaleStated,
    NoRationale,
    /// A counted prior decision stands as stated reasoning (linked, not judged sound).
    PremiseLinked,
    ConfidenceDeclared,
    ConfidenceComparedWithGrounding,
    RestsOnBetOnly,
    RestsOnNothing,
    CounterOptionRecorded,
    NoCounterOption,
    CounterEvidenceRecorded,
    NoCounterEvidence,
    /// How old the prior decisions it rests on were when this one was recorded.
    PremiseAge,
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

/// What a line of attention is about.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionKind {
    /// High confidence declared at capture over a declared bet alone.
    HighConfidenceOverBet,
    /// High confidence declared at capture with nothing on record that the decision rests on.
    HighConfidenceOverNothing,
}

/// Something worth a second look. It is not a deduction: no level moves because of it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Attention {
    pub kind: AttentionKind,
    /// The dimension whose floor found it.
    pub dimension: Dimension,
    pub text: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub node_ids: Vec<String>,
}

/// The seven dimensions for one decision, each standing alone, and what deserves a look.
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
    /// Lines that deserve a look, in a fixed order; empty when nothing does.
    pub attention: Vec<Attention>,
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
const NO_CONFIDENCE_WHY: &str = "No confidence was declared at capture, so there is nothing to compare with what the decision rests on. None is inferred from the wording of the rationale or from how the decision is grounded.";
const NOT_JUDGED_REASONING: &str = "whether the inference is sound is judged, not derived: no higher than partial without a model assessment";

/// The profile of one decision, or `None` when the decision does not exist. Read-only.
pub fn quality_profile_of(
    graph: &impl GraphView,
    decision_id: &str,
) -> Result<Option<QualityProfile>> {
    Ok(get_record_facts(graph, decision_id)?.map(|facts| profile_from_record(&facts)))
}

/// The profile a record's facts support. Pure: the same facts give the same profile.
pub fn profile_from_record(facts: &RecordFacts) -> QualityProfile {
    let rests = Rests::of(facts);
    let (calibration, attention) = calibration(facts, &rests);
    QualityProfile {
        decision_id: facts.decision_id.clone(),
        floor_version: FLOOR_VERSION,
        framing: framing(facts),
        alternatives: alternatives(facts),
        information: information(facts, &rests),
        reasoning: reasoning(facts, &rests),
        values_tradeoffs: Assessment::not_assessed(VALUES_TRADEOFFS_WHY),
        bias_exposure: bias_exposure(facts, &rests),
        calibration,
        attention,
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

/// A node a reason can name.
trait Node {
    fn node_id(&self) -> &str;
}

impl Node for OptionFact {
    fn node_id(&self) -> &str {
        &self.id
    }
}

impl Node for EvidenceFact {
    fn node_id(&self) -> &str {
        &self.id
    }
}

impl Node for PremiseFact {
    fn node_id(&self) -> &str {
        &self.id
    }
}

impl Node for HypothesisFact {
    fn node_id(&self) -> &str {
        &self.id
    }
}

/// The ids of `nodes`, sorted.
fn sorted_ids<T: Node>(nodes: &[&T]) -> Vec<String> {
    let mut ids: Vec<String> = nodes.iter().map(|node| node.node_id().to_owned()).collect();
    ids.sort();
    ids
}

// ---------------------------------------------------------------------------
// What the record rests on (the ex-ante split every floor below shares)
// ---------------------------------------------------------------------------

/// Whether something recorded at `recorded` pre-dates a decision recorded at `decided`. An offset
/// that is not known shows nothing, so it does not.
fn pre_dates(recorded: Option<i64>, decided: Option<i64>) -> bool {
    matches!((recorded, decided), (Some(recorded), Some(decided)) if recorded < decided)
}

/// Grounding counts when it was attached at capture or recorded before the decision (even if it
/// was linked afterwards). Anything else is `later`.
fn counts(added: &GroundingAdded, recorded: Option<i64>, decided: Option<i64>) -> bool {
    matches!(added, GroundingAdded::AtCapture) || pre_dates(recorded, decided)
}

/// Items of one grounding kind: those that count, and those attached too late to.
struct Counted<T> {
    counted: Vec<T>,
    later: Vec<T>,
}

impl<T> Counted<T> {
    fn split(items: impl Iterator<Item = T>, counts: impl Fn(&T) -> bool) -> Self {
        let (counted, later) = items.partition(counts);
        Self { counted, later }
    }
}

/// Everything a decision rests on, split by the ex-ante rule.
struct Rests<'a> {
    evidence: Counted<&'a EvidenceFact>,
    premises: Counted<&'a PremiseFact>,
    assumptions: Counted<&'a HypothesisFact>,
    bets: Counted<&'a HypothesisFact>,
}

impl<'a> Rests<'a> {
    fn of(facts: &'a RecordFacts) -> Self {
        let decided = facts.event_origin;
        let (bets, assumptions): (Vec<&HypothesisFact>, Vec<&HypothesisFact>) = facts
            .hypotheses
            .iter()
            .partition(|hypothesis| hypothesis.kind == HypothesisKind::Bet);
        let hypothesis_counts = |hypothesis: &&HypothesisFact| {
            counts(&hypothesis.added, hypothesis.event_origin, decided)
        };
        Self {
            evidence: Counted::split(facts.evidence.iter(), |item| {
                counts(&item.added, item.event_origin, decided)
            }),
            premises: Counted::split(facts.premises.iter(), |premise| {
                counts(&premise.added, premise.event_origin, decided)
            }),
            assumptions: Counted::split(assumptions.into_iter(), hypothesis_counts),
            bets: Counted::split(bets.into_iter(), hypothesis_counts),
        }
    }

    fn nothing_counted(&self) -> bool {
        self.evidence.counted.is_empty()
            && self.premises.counted.is_empty()
            && self.assumptions.counted.is_empty()
            && self.bets.counted.is_empty()
    }

    fn nothing_later(&self) -> bool {
        self.evidence.later.is_empty()
            && self.premises.later.is_empty()
            && self.assumptions.later.is_empty()
            && self.bets.later.is_empty()
    }

    /// Something other than a bet counts: an observation, a prior decision or an assumption.
    fn grounded(&self) -> bool {
        !self.evidence.counted.is_empty()
            || !self.premises.counted.is_empty()
            || !self.assumptions.counted.is_empty()
    }

    /// The counted hypotheses, assumptions first.
    fn hypotheses(&self) -> impl Iterator<Item = &'a HypothesisFact> + '_ {
        self.assumptions
            .counted
            .iter()
            .chain(self.bets.counted.iter())
            .copied()
    }
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

/// The options set against the one taken: every option other than the chosen one (all of them
/// while none is chosen).
struct Alternatives<'a> {
    others: Vec<&'a OptionFact>,
    chosen: bool,
}

impl<'a> Alternatives<'a> {
    fn of(facts: &'a RecordFacts) -> Self {
        let chosen = facts.chosen_option_id.as_deref();
        Self {
            others: facts
                .options
                .iter()
                .filter(|option| Some(option.id.as_str()) != chosen)
                .collect(),
            chosen: chosen.is_some(),
        }
    }

    fn considered(&self) -> usize {
        self.others.len() + usize::from(self.chosen)
    }

    /// Something was set against the option taken: at least two options are on record.
    fn set_against(&self) -> bool {
        self.considered() >= 2
    }

    fn besides(&self) -> &'static str {
        if self.chosen {
            "besides the chosen option"
        } else {
            "with no option chosen"
        }
    }
}

/// `none` when nothing was set against the option taken, i.e. fewer than two options are on
/// record; `partial` when some alternative carries no description of its own; `solid` when every
/// alternative does.
fn alternatives(facts: &RecordFacts) -> Assessment {
    let against = Alternatives::of(facts);

    if !against.set_against() {
        return Assessment::assessed(
            Level::None,
            vec![reason(
                ReasonKind::NoAlternativeRecorded,
                no_alternative_text(&against).to_owned(),
                Vec::new(),
            )],
        );
    }

    let undescribed: Vec<&OptionFact> = against
        .others
        .iter()
        .copied()
        .filter(|option| !has_own_description(option))
        .collect();
    let mut reasons = vec![reason(
        ReasonKind::AlternativesRecorded,
        format!(
            "{} recorded {}",
            plural(against.others.len(), "alternative"),
            against.besides()
        ),
        sorted_ids(&against.others),
    )];
    let level = if undescribed.is_empty() {
        reasons.push(reason(
            ReasonKind::AlternativesDescribed,
            "every alternative carries a description of its own".to_owned(),
            sorted_ids(&against.others),
        ));
        Level::Solid
    } else {
        reasons.push(reason(
            ReasonKind::AlternativesUndescribed,
            format!(
                "{} without a description of its own (text a capture surface fills in does not count)",
                plural(undescribed.len(), "alternative"),
            ),
            sorted_ids(&undescribed),
        ));
        Level::Partial
    };
    Assessment::assessed(level, reasons)
}

fn no_alternative_text(against: &Alternatives<'_>) -> &'static str {
    if against.considered() == 0 {
        "no option is recorded"
    } else {
        "only one option is recorded: nothing was set against it"
    }
}

// ---------------------------------------------------------------------------
// Information
// ---------------------------------------------------------------------------

/// A "later" reason for one kind of grounding: shown, never counted.
fn later_reason<T: Node>(kind: ReasonKind, noun: &str, later: &[&T]) -> Option<Reason> {
    (!later.is_empty()).then(|| {
        reason(
            kind,
            format!(
                "{} attached after the decision was captured and not recorded before it (later): shown, never counted",
                plural(later.len(), noun),
            ),
            sorted_ids(later),
        )
    })
}

/// When a prior decision was superseded, relative to the decision that rests on it.
#[derive(Clone, Copy)]
enum SupersededWhen {
    Later,
    Before,
    Unknown,
}

/// One reason per class of superseded prior decision, in a fixed order. Facts about what
/// happened to the premise, not about how the decision was made: none of them moves the level.
fn superseded_reasons(facts: &RecordFacts, premises: &[&PremiseFact]) -> Vec<Reason> {
    let mut later = Vec::new();
    let mut before = Vec::new();
    let mut unknown = Vec::new();
    for premise in premises {
        let Some(superseded) = &premise.superseded else {
            continue;
        };
        let when = match (superseded.event_origin, facts.event_origin) {
            (Some(by), Some(decided)) if by > decided => SupersededWhen::Later,
            (Some(by), Some(decided)) if by < decided => SupersededWhen::Before,
            _ => SupersededWhen::Unknown,
        };
        let group = match when {
            SupersededWhen::Later => &mut later,
            SupersededWhen::Before => &mut before,
            SupersededWhen::Unknown => &mut unknown,
        };
        group.push(&premise.id);
        if !superseded.by_id.is_empty() {
            group.push(&superseded.by_id);
        }
    }
    [
        (later, "since superseded (later)"),
        (before, "already superseded when this decision was recorded"),
        (
            unknown,
            "superseded, and whether that was before or after this decision is not recorded",
        ),
    ]
    .into_iter()
    .filter_map(|(ids, prefix)| superseded_reason(ids, prefix))
    .collect()
}

/// The reason for one class of superseded prior decision, when there is one.
fn superseded_reason(ids: Vec<&String>, prefix: &str) -> Option<Reason> {
    if ids.is_empty() {
        return None;
    }
    let mut node_ids: Vec<String> = ids.into_iter().cloned().collect();
    node_ids.sort();
    node_ids.dedup();
    Some(reason(
        ReasonKind::PremiseSuperseded,
        format!(
            "a prior decision it rests on is {prefix}: a fact about what happened next, not about how this was made, so the level does not change"
        ),
        node_ids,
    ))
}

/// From what the decision rests on: evidence, prior decisions, assumptions and declared bets.
/// `none` when none counts; `partial` when something does; `solid` when a counted evidence item
/// says where it was observed, so it can be checked again. A prior decision, an assumption or a
/// bet counts as information on record, not as an observation.
fn information(facts: &RecordFacts, rests: &Rests<'_>) -> Assessment {
    let mut reasons = Vec::new();
    let mut level = Level::Partial;

    if rests.nothing_counted() {
        let text = if rests.nothing_later() {
            "nothing is on record that the decision rests on: no evidence, prior decision, assumption or declared bet"
        } else {
            "nothing that pre-dates the decision is on record: no evidence, prior decision, assumption or declared bet"
        };
        reasons.push(reason(
            ReasonKind::NothingRestedOn,
            text.to_owned(),
            Vec::new(),
        ));
        level = Level::None;
    }

    let evidence = &rests.evidence.counted;
    if !evidence.is_empty() {
        let sourced: Vec<&EvidenceFact> = evidence
            .iter()
            .copied()
            .filter(|item| stated(item.source.as_deref()).is_some())
            .collect();
        reasons.push(reason(
            ReasonKind::EvidenceCounted,
            format!(
                "{} counted: recorded before the decision or attached at capture",
                plural(evidence.len(), "evidence item"),
            ),
            sorted_ids(evidence),
        ));
        if sourced.is_empty() {
            reasons.push(reason(
                ReasonKind::EvidenceSourceMissing,
                "where it was observed is stated for none".to_owned(),
                Vec::new(),
            ));
        } else {
            reasons.push(reason(
                ReasonKind::EvidenceSourceStated,
                format!(
                    "where it was observed is stated for {} of {}",
                    sourced.len(),
                    evidence.len()
                ),
                sorted_ids(&sourced),
            ));
            level = Level::Solid;
        }
    }

    let premises = &rests.premises.counted;
    if !premises.is_empty() {
        reasons.push(reason(
            ReasonKind::PremiseCounted,
            format!(
                "{} counted: recorded before the decision or attached at capture",
                plural(premises.len(), "prior decision"),
            ),
            sorted_ids(premises),
        ));
        reasons.extend(superseded_reasons(facts, premises));
    }
    let assumptions = &rests.assumptions.counted;
    if !assumptions.is_empty() {
        reasons.push(reason(
            ReasonKind::AssumptionCounted,
            format!(
                "{} counted: a stated premise, not yet checked",
                plural(assumptions.len(), "assumption"),
            ),
            sorted_ids(assumptions),
        ));
    }
    let bets = &rests.bets.counted;
    if !bets.is_empty() {
        reasons.push(reason(
            ReasonKind::BetCounted,
            format!(
                "{} declared: an acknowledged unknown, not information gathered",
                plural(bets.len(), "bet"),
            ),
            sorted_ids(bets),
        ));
    }

    reasons.extend(later_reason(
        ReasonKind::EvidenceLater,
        "evidence item",
        &rests.evidence.later,
    ));
    reasons.extend(later_reason(
        ReasonKind::PremiseLater,
        "prior decision",
        &rests.premises.later,
    ));
    reasons.extend(later_reason(
        ReasonKind::AssumptionLater,
        "assumption",
        &rests.assumptions.later,
    ));
    reasons.extend(later_reason(ReasonKind::BetLater, "bet", &rests.bets.later));
    Assessment::assessed(level, reasons)
}

// ---------------------------------------------------------------------------
// Reasoning
// ---------------------------------------------------------------------------

/// `none` when neither a rationale nor a prior decision it follows from is recorded; `partial`
/// when either is. Never higher without a model: whether the inference from information to
/// choice is sound is a judgement.
fn reasoning(facts: &RecordFacts, rests: &Rests<'_>) -> Assessment {
    let rationale = stated(facts.rationale.as_deref()).is_some();
    let premises = &rests.premises.counted;
    let mut reasons = Vec::new();

    if rationale {
        reasons.push(reason(
            ReasonKind::RationaleStated,
            "a rationale is recorded (stated, not judged sound)".to_owned(),
            vec![facts.decision_id.clone()],
        ));
    } else {
        reasons.push(reason(
            ReasonKind::NoRationale,
            "no rationale is recorded".to_owned(),
            Vec::new(),
        ));
    }
    if !premises.is_empty() {
        reasons.push(reason(
            ReasonKind::PremiseLinked,
            format!(
                "it follows from {} (a stated link, not judged sound)",
                plural(premises.len(), "prior decision"),
            ),
            sorted_ids(premises),
        ));
    }
    reasons.extend(later_reason(
        ReasonKind::PremiseLater,
        "prior decision",
        &rests.premises.later,
    ));

    let level = if rationale || !premises.is_empty() {
        reasons.push(reason(
            ReasonKind::NotJudgedWithoutModel,
            NOT_JUDGED_REASONING.to_owned(),
            Vec::new(),
        ));
        Level::Partial
    } else {
        Level::None
    };
    Assessment::assessed(level, reasons)
}

// ---------------------------------------------------------------------------
// Calibration
// ---------------------------------------------------------------------------

/// The confidence vocabulary the capture verbs accept and the classifier extracts.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Confidence {
    Low,
    Medium,
    High,
}

impl Confidence {
    fn parse(text: &str) -> Option<Self> {
        match text.to_ascii_lowercase().as_str() {
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            _ => None,
        }
    }

    fn word(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

/// What a decision rests on, in words, for the calibration comparison.
fn rests_on_summary(rests: &Rests<'_>) -> String {
    let mut parts = Vec::new();
    for (count, noun) in [
        (rests.evidence.counted.len(), "evidence item"),
        (rests.premises.counted.len(), "prior decision"),
        (rests.assumptions.counted.len(), "assumption"),
        (rests.bets.counted.len(), "declared bet"),
    ] {
        if count > 0 {
            parts.push(plural(count, noun));
        }
    }
    parts.join(", ")
}

/// Compares the confidence the decider declared at capture with what the decision rests on.
///
/// Not assessed without a declared confidence, and none is ever inferred. Otherwise `none` when
/// nothing counted is on record to compare it with, and `partial` when something is: whether the
/// confidence matches it is judged, so no higher without a model. The level never depends on
/// the confidence itself. High confidence over a bet alone, or over nothing, adds an attention
/// line instead.
fn calibration(facts: &RecordFacts, rests: &Rests<'_>) -> (Assessment, Vec<Attention>) {
    let Some(declared) = stated(facts.expressed_confidence.as_deref()) else {
        return (Assessment::not_assessed(NO_CONFIDENCE_WHY), Vec::new());
    };
    let Some(confidence) = Confidence::parse(declared) else {
        return (
            Assessment::not_assessed(&format!(
                "The declared confidence '{declared}' is not one of low, medium or high, so it is not compared with what the decision rests on. None is inferred in its place."
            )),
            Vec::new(),
        );
    };

    let mut reasons = vec![reason(
        ReasonKind::ConfidenceDeclared,
        format!(
            "confidence declared at capture: {} (the decider's own words, never system-computed)",
            confidence.word()
        ),
        vec![facts.decision_id.clone()],
    )];
    let mut attention = Vec::new();
    let high = confidence == Confidence::High;

    let level = if rests.grounded() {
        let mut ids = sorted_ids(&rests.evidence.counted);
        ids.extend(sorted_ids(&rests.premises.counted));
        ids.extend(sorted_ids(&rests.assumptions.counted));
        ids.extend(sorted_ids(&rests.bets.counted));
        ids.sort();
        reasons.push(reason(
            ReasonKind::ConfidenceComparedWithGrounding,
            format!(
                "compared with what it rests on: {}",
                rests_on_summary(rests)
            ),
            ids,
        ));
        Level::Partial
    } else if !rests.bets.counted.is_empty() {
        let ids = sorted_ids(&rests.bets.counted);
        reasons.push(reason(
            ReasonKind::RestsOnBetOnly,
            "it rests only on a declared bet: the unknown is acknowledged, and nothing observed or decided stands behind it on record".to_owned(),
            ids.clone(),
        ));
        if high {
            attention.push(Attention {
                kind: AttentionKind::HighConfidenceOverBet,
                dimension: Dimension::Calibration,
                text: "high confidence was declared at capture over a declared bet alone: worth a look at whether it fits (not a deduction)".to_owned(),
                node_ids: ids,
            });
        }
        Level::Partial
    } else {
        let text = if rests.nothing_later() {
            "nothing is on record that the decision rests on, so there is nothing to compare it with".to_owned()
        } else {
            "nothing that pre-dates the decision is on record, so there is nothing to compare it with (what was attached later is shown under Information, never counted)".to_owned()
        };
        reasons.push(reason(ReasonKind::RestsOnNothing, text, Vec::new()));
        if high {
            attention.push(Attention {
                kind: AttentionKind::HighConfidenceOverNothing,
                dimension: Dimension::Calibration,
                text: "high confidence was declared at capture with nothing on record that the decision rests on: worth a look at whether it fits (not a deduction)".to_owned(),
                node_ids: vec![facts.decision_id.clone()],
            });
        }
        Level::None
    };
    if level == Level::Partial {
        reasons.push(reason(
            ReasonKind::NotJudgedWithoutModel,
            "whether the confidence matches what it rests on is judged, not derived: no higher than partial without a model assessment".to_owned(),
            Vec::new(),
        ));
    }
    (Assessment::assessed(level, reasons), attention)
}

// ---------------------------------------------------------------------------
// Bias exposure
// ---------------------------------------------------------------------------

/// Evidence recorded before the decision that refutes something it rests on, even if the link
/// saying so was added afterwards (the same rule as for any evidence: what pre-dates the decision
/// was knowable). Sorted pairs of `(evidence id, hypothesis id)`.
fn counter_evidence<'a>(facts: &RecordFacts, rests: &Rests<'a>) -> Vec<(&'a str, &'a str)> {
    let mut pairs: Vec<(&str, &str)> = Vec::new();
    for hypothesis in rests.hypotheses() {
        for refutation in &hypothesis.refuted_by {
            if pre_dates(refutation.evidence_origin, facts.event_origin) {
                pairs.push((refutation.evidence_id.as_str(), hypothesis.id.as_str()));
            }
        }
    }
    pairs.sort_unstable();
    pairs.dedup();
    pairs
}

/// How old each counted prior decision was, in whole days, when this decision was recorded.
/// Left out when either record has no timestamp or the prior decision is stamped later.
fn premise_ages<'a>(facts: &RecordFacts, premises: &[&'a PremiseFact]) -> Vec<(&'a str, i64)> {
    let Some(decided_at) = facts.occurred_at else {
        return Vec::new();
    };
    let mut ages: Vec<(&str, i64)> = premises
        .iter()
        .filter_map(|premise| {
            let age = decided_at.signed_duration_since(premise.occurred_at?);
            (age >= chrono::Duration::zero()).then(|| (premise.id.as_str(), age.num_days()))
        })
        .collect();
    ages.sort();
    ages
}

/// Whether the choice was exposed to a counter, reported as facts: an option set against it,
/// evidence on record before the decision that refutes something it rests on, and how old the
/// prior decisions it rests on were. `none` when neither a counter-option nor counter-evidence
/// is on record; `partial` when either is. Never higher without a model: whether a distortion
/// shaped the choice is a judgement, and the age of a premise is shown, never scored.
fn bias_exposure(facts: &RecordFacts, rests: &Rests<'_>) -> Assessment {
    let against = Alternatives::of(facts);
    let counter = counter_evidence(facts, rests);
    let mut reasons = Vec::new();

    if against.set_against() {
        reasons.push(reason(
            ReasonKind::CounterOptionRecorded,
            format!(
                "{} recorded {}: something was set against the option taken",
                plural(against.others.len(), "counter-option"),
                against.besides()
            ),
            sorted_ids(&against.others),
        ));
    } else {
        reasons.push(reason(
            ReasonKind::NoCounterOption,
            no_alternative_text(&against).to_owned(),
            Vec::new(),
        ));
    }

    if counter.is_empty() {
        reasons.push(reason(
            ReasonKind::NoCounterEvidence,
            "no evidence that refutes something the decision rests on was on record before it"
                .to_owned(),
            Vec::new(),
        ));
    } else {
        let evidence: BTreeSet<&str> = counter.iter().map(|(evidence, _)| *evidence).collect();
        let node_ids: BTreeSet<&str> = counter
            .iter()
            .flat_map(|(evidence, hypothesis)| [*evidence, *hypothesis])
            .collect();
        reasons.push(reason(
            ReasonKind::CounterEvidenceRecorded,
            format!(
                "{} on record before the decision refutes something it rests on: the choice was exposed to counter-evidence (not that it weighed it)",
                plural(evidence.len(), "evidence item"),
            ),
            node_ids.into_iter().map(str::to_owned).collect(),
        ));
    }

    let premises = &rests.premises.counted;
    if !premises.is_empty() {
        let ages = premise_ages(facts, premises);
        let text = if ages.is_empty() {
            "the age of the prior decisions it rests on when this was recorded cannot be derived: a timestamp is missing".to_owned()
        } else {
            let listed: Vec<String> = ages
                .iter()
                .map(|(id, days)| {
                    let unit = if *days == 1 { "day" } else { "days" };
                    format!("{id} {days} {unit}")
                })
                .collect();
            let missing = premises.len() - ages.len();
            let rest = if missing == 0 {
                String::new()
            } else {
                format!("; not derivable for {missing}")
            };
            format!(
                "age of the prior decisions it rests on when this was recorded: {}{rest}",
                listed.join(", ")
            )
        };
        reasons.push(reason(ReasonKind::PremiseAge, text, sorted_ids(premises)));
    }

    let level = if against.set_against() || !counter.is_empty() {
        reasons.push(reason(
            ReasonKind::NotJudgedWithoutModel,
            "whether a distortion shaped the choice is judged, not derived: no higher than partial without a model assessment".to_owned(),
            Vec::new(),
        ));
        Level::Partial
    } else {
        Level::None
    };
    Assessment::assessed(level, reasons)
}

#[cfg(test)]
pub(crate) mod tests;
