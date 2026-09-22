//! All event types, payload variants, actor/source/relation enums, and the event builder — the vocabulary of the append-only ledger.

use chrono::{DateTime, Utc};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub type EventId = u64;

#[derive(Debug, Clone, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TenantId(String);

impl TenantId {
    pub const LOCAL_VALUE: &'static str = "local";

    pub fn new(value: impl Into<String>) -> std::result::Result<Self, TenantIdError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(TenantIdError::Empty);
        }
        Ok(Self(value))
    }

    pub fn local() -> Self {
        Self(Self::LOCAL_VALUE.to_owned())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for TenantId {
    fn default() -> Self {
        Self::local()
    }
}

impl std::fmt::Display for TenantId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TenantIdError {
    #[error("tenant_id must not be empty")]
    Empty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventType {
    #[serde(rename = "decision.proposed")]
    DecisionProposed,
    #[serde(rename = "decision.requested")]
    DecisionRequested,
    #[serde(rename = "decision.accepted")]
    DecisionAccepted,
    #[serde(rename = "decision.rejected")]
    DecisionRejected,
    #[serde(rename = "decision.superseded")]
    DecisionSuperseded,
    #[serde(rename = "evidence.recorded")]
    EvidenceRecorded,
    #[serde(rename = "hypothesis.recorded")]
    HypothesisRecorded,
    #[serde(rename = "relation.added")]
    RelationAdded,
    #[serde(rename = "relation.removed")]
    RelationRemoved,
    #[serde(rename = "blocker.reported")]
    BlockerReported,
    #[serde(rename = "blocker.resolved")]
    BlockerResolved,
    #[serde(rename = "notification.sent")]
    NotificationSent,
    #[serde(rename = "notification.acknowledged")]
    NotificationAcknowledged,
    #[serde(rename = "ingest.batch_received")]
    IngestBatchReceived,
    #[serde(rename = "ingest.batch_classified")]
    IngestBatchClassified,
    #[serde(rename = "decision.scored")]
    DecisionScored,
    #[serde(rename = "decision.metadata_derived")]
    DecisionMetadataDerived,
    #[serde(rename = "project.registered")]
    ProjectRegistered,
    #[serde(rename = "project.linked")]
    ProjectLinked,
    #[serde(rename = "project.unlinked")]
    ProjectUnlinked,
    #[serde(rename = "project.anchored")]
    ProjectAnchored,
    #[serde(rename = "project.unanchored")]
    ProjectUnanchored,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventSource {
    #[default]
    Cli,
    Agent,
    Human,
    Slack,
    Document,
    Api,
}

impl EventSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Agent => "agent",
            Self::Human => "human",
            Self::Slack => "slack",
            Self::Document => "document",
            Self::Api => "api",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventProvenance {
    pub source: EventSource,
    pub source_ref: Option<String>,
}

impl EventProvenance {
    pub fn new(source: EventSource, source_ref: Option<String>) -> Self {
        Self { source, source_ref }
    }

    pub fn cli() -> Self {
        Self::new(EventSource::Cli, None)
    }

    pub fn agent(source_ref: impl Into<String>) -> Self {
        Self::new(EventSource::Agent, Some(source_ref.into()))
    }

    pub fn human(source_ref: impl Into<String>) -> Self {
        Self::new(EventSource::Human, Some(source_ref.into()))
    }

    pub fn slack(source_ref: impl Into<String>) -> Self {
        Self::new(EventSource::Slack, Some(source_ref.into()))
    }

    pub fn document(source_ref: impl Into<String>) -> Self {
        Self::new(EventSource::Document, Some(source_ref.into()))
    }

    pub fn api(source_ref: Option<String>) -> Self {
        Self::new(EventSource::Api, source_ref)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    #[serde(default = "TenantId::local")]
    pub tenant_id: TenantId,
    pub event_id: Option<EventId>,
    pub event_uuid: Uuid,
    pub correlation_id: Option<String>,
    pub causation_event_id: Option<EventId>,
    #[serde(rename = "type")]
    pub event_type: EventType,
    pub actor_id: String,
    #[serde(default)]
    pub source: EventSource,
    #[serde(default)]
    pub source_ref: Option<String>,
    pub payload: Value,
    pub ts: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionProposedPayload {
    pub decision_id: String,
    pub title: String,
    pub rationale: String,
    #[serde(default)]
    pub topic_keys: Vec<String>,
    #[serde(default)]
    pub option_ids: Vec<String>,
    /// Human-readable label per `option_ids` entry, index-aligned. `#[serde(default)]` so
    /// events from before this field existed still replay: the projector falls back to the
    /// raw option id when a label is missing (see `project_decision_proposed`). The write
    /// layer (`commands::propose_decision_with_id`) requires a label for every option going
    /// forward; this stays optional at the event/replay layer so historical events without it
    /// keep replaying.
    #[serde(default)]
    pub option_labels: Vec<String>,
    /// Optional human-readable description per `option_ids` entry, index-aligned with
    /// `option_labels`. `#[serde(default)]` so events predating this field still replay.
    #[serde(default)]
    pub option_descriptions: Vec<String>,
    pub chosen_option_id: Option<String>,
    #[serde(default)]
    pub hypothesis_ids: Vec<String>,
    #[serde(default)]
    pub evidence_ids: Vec<String>,
    /// Expressed confidence from the decider's own words: low | medium | high. Never system-computed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expressed_confidence: Option<String>,
    /// Verbatim words of the decider, self-contained (not a reference like "1a" into an
    /// external numbered list). Always paired with `question` — see `require_quote_pairing`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote: Option<String>,
    /// The question `quote` answers, spelled out in the capturer's own words. Always paired
    /// with `quote`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionIdPayload {
    pub decision_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionRejectedPayload {
    pub decision_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionSupersededPayload {
    pub old_decision_id: String,
    pub new_decision_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum DecisionBlockerPriority {
    #[serde(rename = "P0", alias = "p0")]
    P0,
    #[serde(rename = "P1", alias = "p1")]
    P1,
    #[serde(rename = "P2", alias = "p2")]
    P2,
    #[serde(rename = "P3", alias = "p3")]
    P3,
    #[serde(rename = "P4", alias = "p4")]
    P4,
}

impl DecisionBlockerPriority {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::P0 => "P0",
            Self::P1 => "P1",
            Self::P2 => "P2",
            Self::P3 => "P3",
            Self::P4 => "P4",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "p0" => Some(Self::P0),
            "p1" => Some(Self::P1),
            "p2" => Some(Self::P2),
            "p3" => Some(Self::P3),
            "p4" => Some(Self::P4),
            _ => None,
        }
    }
}

pub type BlockerPriority = DecisionBlockerPriority;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionRequestedPayload {
    pub topic_keys: Vec<String>,
    pub decision_id: Option<String>,
    pub reason: String,
    pub priority: DecisionBlockerPriority,
    pub required_owner_id: Option<String>,
    pub authority_class: String,
    pub requested_by: String,
    pub client_request_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockerReportedPayload {
    pub blocker_id: String,
    pub blocked_actor_id: String,
    pub decision_id: Option<String>,
    #[serde(default)]
    pub topic_keys: Vec<String>,
    pub blocked_ref: String,
    pub blocked_ref_type: String,
    pub reason: String,
    pub priority: DecisionBlockerPriority,
    pub last_progress_at: Option<DateTime<Utc>>,
    pub required_owner_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationSentPayload {
    pub blocker_id: String,
    pub recipient_actor_id: String,
    pub channel: String,
    pub threshold_rule: String,
    pub source_event_ids: Vec<EventId>,
    pub dedupe_key: String,
    pub sent_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockerResolvedPayload {
    pub blocker_id: String,
    pub resolution_event_id: Option<EventId>,
    pub resolution_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationAcknowledgedPayload {
    pub notification_id: String,
    pub ack_at: DateTime<Utc>,
    pub snooze_until: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceRecordedPayload {
    pub evidence_id: String,
    pub content: String,
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HypothesisRecordedPayload {
    pub hypothesis_id: String,
    pub statement: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RelationKind {
    #[serde(rename = "BASED_ON", alias = "based_on")]
    BasedOn,
    #[serde(rename = "HAS_OPTION", alias = "has_option")]
    HasOption,
    #[serde(rename = "CHOSE", alias = "chose")]
    Chose,
    #[serde(
        rename = "ASSUMES",
        alias = "assumes",
        alias = "PREMISED_ON",
        alias = "premised_on"
    )]
    Assumes,
    #[serde(rename = "SUPPORTS", alias = "supports")]
    Supports,
    #[serde(rename = "REFUTES", alias = "refutes")]
    Refutes,
    #[serde(rename = "SAME_AS", alias = "same_as")]
    SameAs,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IngestTurn {
    pub turn_id: String,
    pub role: String,
    pub text: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IngestBatchReceivedPayload {
    pub batch_id: String,
    pub agent_tool: String,
    pub session_id: String,
    pub turns: Vec<IngestTurn>,
}

/// Deserializes a field that may appear on disk as `null` (none), a bare string
/// (one actor, the pre-widening shape), or an array of strings (current shape) —
/// so ledger events written before `accepted_by`/`rejected_by` widened from
/// `Option<String>` to `Vec<String>` still parse into the same Rust type.
fn string_or_seq<'de, D>(deserializer: D) -> std::result::Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct StringOrSeq;

    impl<'de> serde::de::Visitor<'de> for StringOrSeq {
        type Value = Vec<String>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("null, a string, or an array of strings")
        }

        fn visit_unit<E>(self) -> std::result::Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(Vec::new())
        }

        fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(vec![value.to_owned()])
        }

        fn visit_seq<A>(self, mut seq: A) -> std::result::Result<Self::Value, A::Error>
        where
            A: serde::de::SeqAccess<'de>,
        {
            let mut values = Vec::new();
            while let Some(value) = seq.next_element::<String>()? {
                values.push(value);
            }
            Ok(values)
        }
    }

    deserializer.deserialize_any(StringOrSeq)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureItem {
    pub kind: String,
    pub title: String,
    pub rationale: String,
    pub topic_keys: Vec<String>,
    pub evidence_ids: Vec<String>,
    pub options: Option<Vec<String>>,
    pub chosen_option: Option<String>,
    /// Haiku extractor's self-estimate that this capture was correctly extracted; not the decision Quality score.
    pub extraction_confidence: f64,
    /// Expressed confidence from the decider's own words: low | medium | high. Never system-computed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expressed_confidence: Option<String>,
    /// ID of the decision being superseded; only when present in the input text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes_id: Option<String>,
    /// Hypothesis IDs this decision is premised on; only IDs present in the input.
    #[serde(default, skip_serializing_if = "Vec::is_empty", alias = "assumes_ids")]
    pub premised_on_ids: Vec<String>,
    /// Hypothesis IDs this evidence supports; only IDs present in the input.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supports_ids: Vec<String>,
    /// Hypothesis IDs this evidence refutes; only IDs present in the input.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refutes_ids: Vec<String>,
    /// Actor who proposed/made/reported this item, named in the input text. Never infer from context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_id: Option<String>,
    /// Actors who accepted this decision (or decision request), named in the input text.
    /// Deserializes `null`, a bare string, or an array from stored ledger events so old and
    /// new `ingest.batch_classified` payloads both parse into the same shape.
    #[serde(
        default,
        deserialize_with = "string_or_seq",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub accepted_by: Vec<String>,
    /// Actors who rejected this decision (or decision request), named in the input text.
    /// Deserializes `null`, a bare string, or an array from stored ledger events so old and
    /// new `ingest.batch_classified` payloads both parse into the same shape.
    #[serde(
        default,
        deserialize_with = "string_or_seq",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub rejected_by: Vec<String>,
    /// For blocker captures: the actor being blocked, named in the input text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_actor_id: Option<String>,
    /// For blocker captures: the decision ID being blocked; only if present in the input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_id: Option<String>,
    /// All actor IDs that participated in the session producing this capture (human + agent).
    /// Auto-populated from batch metadata; not LLM-extracted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub participants: Vec<String>,
    /// Actor ID of whoever initiated the session (the batch submitter).
    /// Auto-populated from batch metadata; not LLM-extracted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_initiator: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IngestBatchClassifiedPayload {
    pub batch_id: String,
    pub classifier_model: String,
    pub schema_version: String,
    pub captures: Vec<CaptureItem>,
}

/// One scored quality dimension: score in [0,1] plus a human-readable explanation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QualityDim {
    /// Score in [0,1] for this dimension.
    pub score: f64,
    pub explanation: String,
}

/// All seven Quality dimensions assessed ex ante.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QualityDims {
    pub framing: QualityDim,
    pub alternatives: QualityDim,
    pub information: QualityDim,
    pub reasoning: QualityDim,
    pub values_tradeoffs: QualityDim,
    pub bias_exposure: QualityDim,
    pub calibration: QualityDim,
}

/// Importance factors stored individually (Importance = stakes × irreversibility × actionability).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportanceFactors {
    /// Unbounded positive magnitude (log-scaled; severity × reach).
    pub stakes: f64,
    pub stakes_explanation: String,
    /// Irreversibility discount in [0,1]: 0 = fully reversible, 1 = fully irreversible.
    pub irreversibility: f64,
    pub irreversibility_explanation: String,
    /// Actionability gate in [0,1]: 0 = not actionable, 1 = fully actionable.
    pub actionability: f64,
    pub actionability_explanation: String,
}

/// Payload for a `decision.scored` append-only annotation event.
///
/// Scores are Layer-3: server-computed, stored separately from the decision,
/// never an edit to it. Re-assessments append a new event with `supersedes_score_id`
/// pointing at the prior one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionScoredPayload {
    /// The capture node ID (e.g. `capture:42:0`) from the classified batch.
    pub capture_node_id: String,
    pub scorer_model: String,
    /// Version tag for the quality dimension weights (e.g. "v1") so composites can recompute.
    pub weight_version: String,
    /// event_uuid of a prior `decision.scored` event that this assessment supersedes, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes_score_id: Option<String>,
    /// Per-dimension Quality scores [0,1] with explanations.
    pub quality_dims: QualityDims,
    /// Importance factors (stakes × irreversibility × actionability).
    pub importance: ImportanceFactors,
}

/// Provenance for a derived attribute: was it explicitly stated or inferred?
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    Stated,
    Derived,
}

/// Disposition of a decision actor toward an option or constraint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DispositionValue {
    Willing,
    Constrained,
    Unwilling,
}

/// Kind of cross-track surface referenced by a decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceKind {
    Resource,
    Interface,
    Schema,
    Constraint,
    Concept,
}

/// A premise extracted from a captured decision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PremiseAttribute {
    pub statement: String,
    pub provenance: Provenance,
    pub confidence: f64,
}

/// A foreclosed option extracted from a captured decision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForeclosedOptionAttribute {
    pub description: String,
    pub provenance: Provenance,
    pub confidence: f64,
}

/// A goal extracted from a captured decision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalAttribute {
    pub statement: String,
    pub provenance: Provenance,
    pub confidence: f64,
}

/// Disposition annotation on a captured decision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispositionAttribute {
    pub value: DispositionValue,
    pub provenance: Provenance,
    pub confidence: f64,
}

/// A surface (resource, interface, schema, constraint, concept) that a decision touches.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrossTrackSurface {
    pub surface: String,
    pub kind: SurfaceKind,
    pub provenance: Provenance,
    pub confidence: f64,
}

/// Payload for a `decision.metadata_derived` append-only annotation event.
///
/// Layer-3: server- or agent-computed derived metadata stored separately from
/// the decision, never an edit to it. Re-derivations append a new event with
/// `supersedes_derivation_id` pointing at the prior one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionMetadataDerivedPayload {
    pub decision_id: String,
    pub derivation_model: String,
    pub schema_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes_derivation_id: Option<String>,
    #[serde(default)]
    pub premises: Vec<PremiseAttribute>,
    #[serde(default)]
    pub foreclosed_options: Vec<ForeclosedOptionAttribute>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disposition: Option<DispositionAttribute>,
    #[serde(default)]
    pub goals: Vec<GoalAttribute>,
    #[serde(default)]
    pub cross_track_surfaces: Vec<CrossTrackSurface>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelationAddedPayload {
    pub relation: RelationKind,
    pub from_id: String,
    pub to_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelationRemovedPayload {
    pub relation: RelationKind,
    pub from_id: String,
    pub to_id: String,
}

/// A project's own handle is never `part_of`/`depends_on` another project's handle by
/// accident: the two kinds are wire-distinct so a link event can never be mistaken for a
/// decision-graph `RelationKind` (see `events::RelationKind` above), even though both use
/// the word "relation" informally.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProjectLinkKind {
    #[serde(rename = "part_of")]
    PartOf,
    #[serde(rename = "depends_on")]
    DependsOn,
}

impl ProjectLinkKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PartOf => "part_of",
            Self::DependsOn => "depends_on",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProjectAnchorKind {
    #[serde(rename = "folder")]
    Folder,
    #[serde(rename = "rig")]
    Rig,
    #[serde(rename = "jira")]
    Jira,
    #[serde(rename = "linear")]
    Linear,
    #[serde(rename = "github")]
    Github,
    #[serde(rename = "channel")]
    Channel,
}

impl ProjectAnchorKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Folder => "folder",
            Self::Rig => "rig",
            Self::Jira => "jira",
            Self::Linear => "linear",
            Self::Github => "github",
            Self::Channel => "channel",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectRegisteredPayload {
    pub handle: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
}

/// Shared shape for both `project.linked` and `project.unlinked` — an unlink is the same
/// fact recorded again, not a mutation of the original (nothing is ever deleted from the
/// ledger; see AGENTS.md section 1, "forget responsibly").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectLinkPayload {
    pub from: String,
    pub to: String,
    pub kind: ProjectLinkKind,
}

/// Shared shape for both `project.anchored` and `project.unanchored`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectAnchorPayload {
    pub handle: String,
    pub anchor_kind: ProjectAnchorKind,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EventPayload {
    DecisionProposed(DecisionProposedPayload),
    DecisionRequested(DecisionRequestedPayload),
    DecisionAccepted(DecisionIdPayload),
    DecisionRejected(DecisionRejectedPayload),
    DecisionSuperseded(DecisionSupersededPayload),
    EvidenceRecorded(EvidenceRecordedPayload),
    HypothesisRecorded(HypothesisRecordedPayload),
    RelationAdded(RelationAddedPayload),
    RelationRemoved(RelationRemovedPayload),
    BlockerReported(BlockerReportedPayload),
    BlockerResolved(BlockerResolvedPayload),
    NotificationSent(NotificationSentPayload),
    NotificationAcknowledged(NotificationAcknowledgedPayload),
    IngestBatchReceived(IngestBatchReceivedPayload),
    IngestBatchClassified(IngestBatchClassifiedPayload),
    DecisionScored(DecisionScoredPayload),
    DecisionMetadataDerived(DecisionMetadataDerivedPayload),
    ProjectRegistered(ProjectRegisteredPayload),
    ProjectLinked(ProjectLinkPayload),
    ProjectUnlinked(ProjectLinkPayload),
    ProjectAnchored(ProjectAnchorPayload),
    ProjectUnanchored(ProjectAnchorPayload),
}

impl EventPayload {
    pub fn event_type(&self) -> EventType {
        match self {
            Self::DecisionProposed(_) => EventType::DecisionProposed,
            Self::DecisionRequested(_) => EventType::DecisionRequested,
            Self::DecisionAccepted(_) => EventType::DecisionAccepted,
            Self::DecisionRejected(_) => EventType::DecisionRejected,
            Self::DecisionSuperseded(_) => EventType::DecisionSuperseded,
            Self::EvidenceRecorded(_) => EventType::EvidenceRecorded,
            Self::HypothesisRecorded(_) => EventType::HypothesisRecorded,
            Self::RelationAdded(_) => EventType::RelationAdded,
            Self::RelationRemoved(_) => EventType::RelationRemoved,
            Self::BlockerReported(_) => EventType::BlockerReported,
            Self::BlockerResolved(_) => EventType::BlockerResolved,
            Self::NotificationSent(_) => EventType::NotificationSent,
            Self::NotificationAcknowledged(_) => EventType::NotificationAcknowledged,
            Self::IngestBatchReceived(_) => EventType::IngestBatchReceived,
            Self::IngestBatchClassified(_) => EventType::IngestBatchClassified,
            Self::DecisionScored(_) => EventType::DecisionScored,
            Self::DecisionMetadataDerived(_) => EventType::DecisionMetadataDerived,
            Self::ProjectRegistered(_) => EventType::ProjectRegistered,
            Self::ProjectLinked(_) => EventType::ProjectLinked,
            Self::ProjectUnlinked(_) => EventType::ProjectUnlinked,
            Self::ProjectAnchored(_) => EventType::ProjectAnchored,
            Self::ProjectUnanchored(_) => EventType::ProjectUnanchored,
        }
    }

    pub fn to_value(&self) -> std::result::Result<Value, serde_json::Error> {
        match self {
            Self::DecisionProposed(payload) => serde_json::to_value(payload),
            Self::DecisionRequested(payload) => serde_json::to_value(payload),
            Self::DecisionAccepted(payload) => serde_json::to_value(payload),
            Self::DecisionRejected(payload) => serde_json::to_value(payload),
            Self::DecisionSuperseded(payload) => serde_json::to_value(payload),
            Self::EvidenceRecorded(payload) => serde_json::to_value(payload),
            Self::HypothesisRecorded(payload) => serde_json::to_value(payload),
            Self::RelationAdded(payload) => serde_json::to_value(payload),
            Self::RelationRemoved(payload) => serde_json::to_value(payload),
            Self::BlockerReported(payload) => serde_json::to_value(payload),
            Self::BlockerResolved(payload) => serde_json::to_value(payload),
            Self::NotificationSent(payload) => serde_json::to_value(payload),
            Self::NotificationAcknowledged(payload) => serde_json::to_value(payload),
            Self::IngestBatchReceived(payload) => serde_json::to_value(payload),
            Self::IngestBatchClassified(payload) => serde_json::to_value(payload),
            Self::DecisionScored(payload) => serde_json::to_value(payload),
            Self::DecisionMetadataDerived(payload) => serde_json::to_value(payload),
            Self::ProjectRegistered(payload) => serde_json::to_value(payload),
            Self::ProjectLinked(payload) => serde_json::to_value(payload),
            Self::ProjectUnlinked(payload) => serde_json::to_value(payload),
            Self::ProjectAnchored(payload) => serde_json::to_value(payload),
            Self::ProjectUnanchored(payload) => serde_json::to_value(payload),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EventEnvelope {
    event_type: EventType,
    payload: EventPayload,
}

impl EventEnvelope {
    pub fn new(payload: EventPayload) -> Self {
        let event_type = payload.event_type();
        Self {
            event_type,
            payload,
        }
    }

    pub fn event_type(&self) -> EventType {
        self.event_type
    }

    pub fn payload(&self) -> &EventPayload {
        &self.payload
    }

    pub fn into_payload(self) -> EventPayload {
        self.payload
    }
}

impl From<EventPayload> for EventEnvelope {
    fn from(payload: EventPayload) -> Self {
        Self::new(payload)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EventBuildError {
    #[error("payload for event type {event_type:?} could not be serialized: {source}")]
    PayloadSerialization {
        event_type: EventType,
        #[source]
        source: serde_json::Error,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct EventBuilder {
    tenant_id: TenantId,
    event_id: Option<EventId>,
    event_uuid: Uuid,
    correlation_id: Option<String>,
    causation_event_id: Option<EventId>,
    actor_id: String,
    source: EventSource,
    source_ref: Option<String>,
    envelope: EventEnvelope,
    ts: Option<DateTime<Utc>>,
}

impl EventBuilder {
    pub fn new(
        event_uuid: Uuid,
        actor_id: impl Into<String>,
        payload: impl Into<EventEnvelope>,
    ) -> Self {
        Self {
            tenant_id: TenantId::local(),
            event_id: None,
            event_uuid,
            correlation_id: None,
            causation_event_id: None,
            actor_id: actor_id.into(),
            source: EventSource::default(),
            source_ref: None,
            envelope: payload.into(),
            ts: None,
        }
    }

    pub fn event_id(mut self, event_id: Option<EventId>) -> Self {
        self.event_id = event_id;
        self
    }

    pub fn tenant_id(mut self, tenant_id: TenantId) -> Self {
        self.tenant_id = tenant_id;
        self
    }

    pub fn correlation_id(mut self, correlation_id: Option<String>) -> Self {
        self.correlation_id = correlation_id;
        self
    }

    pub fn causation_event_id(mut self, causation_event_id: Option<EventId>) -> Self {
        self.causation_event_id = causation_event_id;
        self
    }

    pub fn provenance(mut self, provenance: EventProvenance) -> Self {
        self.source = provenance.source;
        self.source_ref = provenance.source_ref;
        self
    }

    pub fn timestamp(mut self, ts: Option<DateTime<Utc>>) -> Self {
        self.ts = ts;
        self
    }

    pub fn build(self) -> std::result::Result<Event, EventBuildError> {
        let event_type = self.envelope.event_type();
        let payload = self
            .envelope
            .into_payload()
            .to_value()
            .map_err(|source| EventBuildError::PayloadSerialization { event_type, source })?;

        Ok(Event {
            tenant_id: self.tenant_id,
            event_id: self.event_id,
            event_uuid: self.event_uuid,
            correlation_id: self.correlation_id,
            causation_event_id: self.causation_event_id,
            event_type,
            actor_id: self.actor_id,
            source: self.source,
            source_ref: self.source_ref,
            payload,
            ts: self.ts,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EventValidationError {
    #[error("event_id must be positive when present")]
    InvalidEventId,

    #[error("causation_event_id must be positive when present")]
    InvalidCausationEventId,

    #[error("{0} must not be empty")]
    EmptyField(&'static str),

    #[error("{0} contains an empty value")]
    EmptyListValue(&'static str),

    #[error("{0} must contain at least one value")]
    EmptyList(&'static str),

    #[error("{0} contains a non-positive event id")]
    InvalidEventIdListValue(&'static str),

    #[error("{0} requires {1} — a verbatim quote with no stated question, or a question with no quote, is unreadable once the source conversation is gone")]
    RequiresPairedField(&'static str, &'static str),

    #[error("payload does not match event type {event_type:?}: {source}")]
    Payload {
        event_type: EventType,
        #[source]
        source: serde_json::Error,
    },
}

pub fn validate(event: &Event) -> std::result::Result<EventPayload, EventValidationError> {
    validate_common(event)?;

    match event.event_type {
        EventType::DecisionProposed => {
            let payload: DecisionProposedPayload = parse_payload(event)?;
            require_non_empty("payload.decision_id", &payload.decision_id)?;
            require_non_empty("payload.title", &payload.title)?;
            require_non_empty("payload.rationale", &payload.rationale)?;
            require_non_empty_values("payload.topic_keys", &payload.topic_keys)?;
            require_non_empty_values("payload.option_ids", &payload.option_ids)?;
            require_non_empty_values("payload.option_labels", &payload.option_labels)?;
            require_non_empty_values("payload.option_descriptions", &payload.option_descriptions)?;
            require_optional_non_empty(
                "payload.chosen_option_id",
                payload.chosen_option_id.as_deref(),
            )?;
            require_non_empty_values("payload.hypothesis_ids", &payload.hypothesis_ids)?;
            require_non_empty_values("payload.evidence_ids", &payload.evidence_ids)?;
            require_optional_non_empty("payload.quote", payload.quote.as_deref())?;
            require_optional_non_empty("payload.question", payload.question.as_deref())?;
            if payload.quote.is_some() != payload.question.is_some() {
                let (present, missing) = if payload.quote.is_some() {
                    ("payload.quote", "payload.question")
                } else {
                    ("payload.question", "payload.quote")
                };
                return Err(EventValidationError::RequiresPairedField(present, missing));
            }
            Ok(EventPayload::DecisionProposed(payload))
        }
        EventType::DecisionRequested => {
            require_event_provenance(event)?;
            let payload: DecisionRequestedPayload = parse_payload(event)?;
            require_non_empty_list("payload.topic_keys", &payload.topic_keys)?;
            require_non_empty_values("payload.topic_keys", &payload.topic_keys)?;
            require_optional_non_empty("payload.decision_id", payload.decision_id.as_deref())?;
            require_non_empty("payload.reason", &payload.reason)?;
            require_optional_non_empty(
                "payload.required_owner_id",
                payload.required_owner_id.as_deref(),
            )?;
            require_non_empty("payload.authority_class", &payload.authority_class)?;
            require_non_empty("payload.requested_by", &payload.requested_by)?;
            require_non_empty("payload.client_request_id", &payload.client_request_id)?;
            Ok(EventPayload::DecisionRequested(payload))
        }
        EventType::DecisionAccepted => {
            let payload: DecisionIdPayload = parse_payload(event)?;
            require_non_empty("payload.decision_id", &payload.decision_id)?;
            Ok(EventPayload::DecisionAccepted(payload))
        }
        EventType::DecisionRejected => {
            let payload: DecisionRejectedPayload = parse_payload(event)?;
            require_non_empty("payload.decision_id", &payload.decision_id)?;
            require_optional_non_empty("payload.reason", payload.reason.as_deref())?;
            Ok(EventPayload::DecisionRejected(payload))
        }
        EventType::DecisionSuperseded => {
            let payload: DecisionSupersededPayload = parse_payload(event)?;
            require_non_empty("payload.old_decision_id", &payload.old_decision_id)?;
            require_non_empty("payload.new_decision_id", &payload.new_decision_id)?;
            Ok(EventPayload::DecisionSuperseded(payload))
        }
        EventType::EvidenceRecorded => {
            let payload: EvidenceRecordedPayload = parse_payload(event)?;
            require_non_empty("payload.evidence_id", &payload.evidence_id)?;
            require_non_empty("payload.content", &payload.content)?;
            require_optional_non_empty("payload.source", payload.source.as_deref())?;
            Ok(EventPayload::EvidenceRecorded(payload))
        }
        EventType::HypothesisRecorded => {
            let payload: HypothesisRecordedPayload = parse_payload(event)?;
            require_non_empty("payload.hypothesis_id", &payload.hypothesis_id)?;
            require_non_empty("payload.statement", &payload.statement)?;
            Ok(EventPayload::HypothesisRecorded(payload))
        }
        EventType::RelationAdded => {
            let payload: RelationAddedPayload = parse_payload(event)?;
            require_non_empty("payload.from_id", &payload.from_id)?;
            require_non_empty("payload.to_id", &payload.to_id)?;
            Ok(EventPayload::RelationAdded(payload))
        }
        EventType::RelationRemoved => {
            let payload: RelationRemovedPayload = parse_payload(event)?;
            require_non_empty("payload.from_id", &payload.from_id)?;
            require_non_empty("payload.to_id", &payload.to_id)?;
            Ok(EventPayload::RelationRemoved(payload))
        }
        EventType::BlockerReported => {
            require_event_provenance(event)?;
            let payload: BlockerReportedPayload = parse_payload(event)?;
            require_non_empty("payload.blocker_id", &payload.blocker_id)?;
            require_non_empty("payload.blocked_actor_id", &payload.blocked_actor_id)?;
            require_optional_non_empty("payload.decision_id", payload.decision_id.as_deref())?;
            require_non_empty_values("payload.topic_keys", &payload.topic_keys)?;
            if payload.decision_id.is_none() && payload.topic_keys.is_empty() {
                return Err(EventValidationError::EmptyField(
                    "payload.decision_id_or_topic_keys",
                ));
            }
            require_non_empty("payload.blocked_ref", &payload.blocked_ref)?;
            require_non_empty("payload.blocked_ref_type", &payload.blocked_ref_type)?;
            require_non_empty("payload.reason", &payload.reason)?;
            require_optional_non_empty(
                "payload.required_owner_id",
                payload.required_owner_id.as_deref(),
            )?;
            Ok(EventPayload::BlockerReported(payload))
        }
        EventType::BlockerResolved => {
            require_event_provenance(event)?;
            let payload: BlockerResolvedPayload = parse_payload(event)?;
            require_non_empty("payload.blocker_id", &payload.blocker_id)?;
            require_optional_non_empty(
                "payload.resolution_reason",
                payload.resolution_reason.as_deref(),
            )?;
            if payload.resolution_event_id.is_none() && payload.resolution_reason.is_none() {
                return Err(EventValidationError::EmptyField(
                    "payload.resolution_event_id_or_resolution_reason",
                ));
            }
            Ok(EventPayload::BlockerResolved(payload))
        }
        EventType::NotificationSent => {
            require_event_provenance(event)?;
            let payload: NotificationSentPayload = parse_payload(event)?;
            require_non_empty("payload.blocker_id", &payload.blocker_id)?;
            require_non_empty("payload.recipient_actor_id", &payload.recipient_actor_id)?;
            require_non_empty("payload.channel", &payload.channel)?;
            require_non_empty("payload.threshold_rule", &payload.threshold_rule)?;
            require_non_empty_event_ids("payload.source_event_ids", &payload.source_event_ids)?;
            require_non_empty("payload.dedupe_key", &payload.dedupe_key)?;
            Ok(EventPayload::NotificationSent(payload))
        }
        EventType::NotificationAcknowledged => {
            require_event_provenance(event)?;
            let payload: NotificationAcknowledgedPayload = parse_payload(event)?;
            require_non_empty("payload.notification_id", &payload.notification_id)?;
            Ok(EventPayload::NotificationAcknowledged(payload))
        }
        EventType::IngestBatchReceived => {
            let payload: IngestBatchReceivedPayload = parse_payload(event)?;
            require_non_empty("payload.batch_id", &payload.batch_id)?;
            require_non_empty("payload.agent_tool", &payload.agent_tool)?;
            require_non_empty("payload.session_id", &payload.session_id)?;
            Ok(EventPayload::IngestBatchReceived(payload))
        }
        EventType::IngestBatchClassified => {
            let payload: IngestBatchClassifiedPayload = parse_payload(event)?;
            require_non_empty("payload.batch_id", &payload.batch_id)?;
            require_non_empty("payload.classifier_model", &payload.classifier_model)?;
            require_non_empty("payload.schema_version", &payload.schema_version)?;
            Ok(EventPayload::IngestBatchClassified(payload))
        }
        EventType::DecisionScored => {
            let payload: DecisionScoredPayload = parse_payload(event)?;
            require_non_empty("payload.capture_node_id", &payload.capture_node_id)?;
            require_non_empty("payload.scorer_model", &payload.scorer_model)?;
            require_non_empty("payload.weight_version", &payload.weight_version)?;
            Ok(EventPayload::DecisionScored(payload))
        }
        EventType::DecisionMetadataDerived => {
            let payload: DecisionMetadataDerivedPayload = parse_payload(event)?;
            require_non_empty("payload.decision_id", &payload.decision_id)?;
            require_non_empty("payload.derivation_model", &payload.derivation_model)?;
            require_non_empty("payload.schema_version", &payload.schema_version)?;
            Ok(EventPayload::DecisionMetadataDerived(payload))
        }
        EventType::ProjectRegistered => {
            let payload: ProjectRegisteredPayload = parse_payload(event)?;
            require_non_empty("payload.handle", &payload.handle)?;
            require_optional_non_empty("payload.display_name", payload.display_name.as_deref())?;
            require_optional_non_empty("payload.purpose", payload.purpose.as_deref())?;
            Ok(EventPayload::ProjectRegistered(payload))
        }
        EventType::ProjectLinked => {
            let payload: ProjectLinkPayload = parse_payload(event)?;
            require_non_empty("payload.from", &payload.from)?;
            require_non_empty("payload.to", &payload.to)?;
            Ok(EventPayload::ProjectLinked(payload))
        }
        EventType::ProjectUnlinked => {
            let payload: ProjectLinkPayload = parse_payload(event)?;
            require_non_empty("payload.from", &payload.from)?;
            require_non_empty("payload.to", &payload.to)?;
            Ok(EventPayload::ProjectUnlinked(payload))
        }
        EventType::ProjectAnchored => {
            let payload: ProjectAnchorPayload = parse_payload(event)?;
            require_non_empty("payload.handle", &payload.handle)?;
            require_non_empty("payload.value", &payload.value)?;
            Ok(EventPayload::ProjectAnchored(payload))
        }
        EventType::ProjectUnanchored => {
            let payload: ProjectAnchorPayload = parse_payload(event)?;
            require_non_empty("payload.handle", &payload.handle)?;
            require_non_empty("payload.value", &payload.value)?;
            Ok(EventPayload::ProjectUnanchored(payload))
        }
    }
}

fn validate_common(event: &Event) -> std::result::Result<(), EventValidationError> {
    if matches!(event.event_id, Some(0)) {
        return Err(EventValidationError::InvalidEventId);
    }

    if matches!(event.causation_event_id, Some(0)) {
        return Err(EventValidationError::InvalidCausationEventId);
    }

    require_non_empty("actor_id", &event.actor_id)?;
    require_non_empty("tenant_id", event.tenant_id.as_str())?;
    require_optional_non_empty("source_ref", event.source_ref.as_deref())?;
    require_optional_non_empty("correlation_id", event.correlation_id.as_deref())
}

fn require_event_provenance(event: &Event) -> std::result::Result<(), EventValidationError> {
    require_present_non_empty("source_ref", event.source_ref.as_deref())?;
    require_present_non_empty("correlation_id", event.correlation_id.as_deref())
}

fn parse_payload<T>(event: &Event) -> std::result::Result<T, EventValidationError>
where
    T: DeserializeOwned,
{
    serde_json::from_value(event.payload.clone()).map_err(|source| EventValidationError::Payload {
        event_type: event.event_type,
        source,
    })
}

fn require_non_empty(
    field: &'static str,
    value: &str,
) -> std::result::Result<(), EventValidationError> {
    if value.trim().is_empty() {
        Err(EventValidationError::EmptyField(field))
    } else {
        Ok(())
    }
}

fn require_optional_non_empty(
    field: &'static str,
    value: Option<&str>,
) -> std::result::Result<(), EventValidationError> {
    if value.is_some_and(|value| value.trim().is_empty()) {
        Err(EventValidationError::EmptyField(field))
    } else {
        Ok(())
    }
}

fn require_present_non_empty(
    field: &'static str,
    value: Option<&str>,
) -> std::result::Result<(), EventValidationError> {
    match value {
        Some(value) => require_non_empty(field, value),
        None => Err(EventValidationError::EmptyField(field)),
    }
}

fn require_non_empty_list(
    field: &'static str,
    values: &[String],
) -> std::result::Result<(), EventValidationError> {
    if values.is_empty() {
        Err(EventValidationError::EmptyList(field))
    } else {
        Ok(())
    }
}

fn require_non_empty_values(
    field: &'static str,
    values: &[String],
) -> std::result::Result<(), EventValidationError> {
    if values.iter().any(|value| value.trim().is_empty()) {
        Err(EventValidationError::EmptyListValue(field))
    } else {
        Ok(())
    }
}

fn require_non_empty_event_ids(
    field: &'static str,
    values: &[EventId],
) -> std::result::Result<(), EventValidationError> {
    if values.is_empty() {
        return Err(EventValidationError::EmptyList(field));
    }
    if values.contains(&0) {
        return Err(EventValidationError::InvalidEventIdListValue(field));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
