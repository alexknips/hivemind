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
    /// A question that decisions answer, recorded as its own node so several decisions answering
    /// the same question are findable deterministically (hivemind-zdsh.16).
    #[serde(rename = "question.recorded")]
    QuestionRecorded,
    /// An explicit ask: someone named a question before any decision answered it. Distinct from
    /// `question.recorded`, which a capture or `ground --answers` also writes implicitly whenever
    /// it names a question nobody has recorded yet — `question.asked` only ever comes from
    /// `hivemind ask` / MCP `request_decision`, so a `Question` node can exist with no explicit
    /// ask, and a question can be asked more than once (hivemind-bbnw.4).
    #[serde(rename = "question.asked")]
    QuestionAsked,
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
    /// A finding (an attention finding from the quality profile) was shown to a recipient over
    /// a channel. The suggestion-side twin of `notification.sent`: its subject is a finding,
    /// not a blocker, so it never draws a notification-for-blocker fact. A
    /// `notification.acknowledged` naming this event's uuid is the acknowledgement
    /// (hivemind-m306.4.2).
    #[serde(rename = "suggestion.surfaced")]
    SuggestionSurfaced,
    #[serde(rename = "ingest.batch_received")]
    IngestBatchReceived,
    #[serde(rename = "ingest.batch_classified")]
    IngestBatchClassified,
    #[serde(rename = "decision.scored")]
    DecisionScored,
    #[serde(rename = "decision.metadata_derived")]
    DecisionMetadataDerived,
    /// A decision changed which project it belongs to. Recorded and reversible: a reversal
    /// is another `decision.moved` with `from`/`to` swapped, never a rewrite of this one
    /// (approved record shape, item 3; hivemind-s15q.10).
    #[serde(rename = "decision.moved")]
    DecisionMoved,
    /// A decision's title changed after capture. Recorded and reversible: a reversal is
    /// another `decision.retitled` with `from`/`to` swapped, never a rewrite of this one
    /// (same shape as `decision.moved`; hivemind-ydmp).
    #[serde(rename = "decision.retitled")]
    DecisionRetitled,
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
    /// A registered project's topic vocabulary grew by one key. A project's vocabulary is
    /// what replaying these yields; nothing ever removes a key (hivemind-zywz).
    #[serde(rename = "project.topic_declared")]
    ProjectTopicDeclared,
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
    /// Registered project handle this decision was filed under. `#[serde(default)]` so
    /// every event predating this field still parses; a missing value means the decision
    /// projects to the recorder's personal project (see `commands::validate_project_handle`
    /// and `personal_project_handle`), never a migration or rewrite of old events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// How `project` was determined. The write layer always records one of these for a
    /// freshly-written event (`Stated` when a handle was given, `PersonalFallback`
    /// otherwise) — `None` only ever appears on events written before this field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_source: Option<ProjectSource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionAcceptedPayload {
    pub decision_id: String,
    /// The human whose delegated scope an agent's self-acceptance falls within
    /// (hivemind-zdsh.6). Present only when the accepting actor is an agent accepting a
    /// decision it proposed itself; absent means "not decided under a stated delegation",
    /// which for an agent self-accept is the agent-decided-alone case. `#[serde(default)]`
    /// so every event predating this field still parses and replays unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delegated_by: Option<String>,
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

/// `suggestion.surfaced` (hivemind-m306.4.2). `finding_id` is the stable id a finding carries
/// (a hash of its kind, node ids and basis time), `decision_id` the decision it is about and
/// `channel` a label the consumer chose: HiveMind does not know what any channel means. The
/// event's own uuid is the notification id a later `notification.acknowledged` names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuggestionSurfacedPayload {
    pub finding_id: String,
    pub decision_id: String,
    pub recipient_actor_id: String,
    pub channel: String,
    pub sent_at: DateTime<Utc>,
}

/// What an acknowledgement says was done with the notification or suggestion it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AckAction {
    /// Looked at, nothing more claimed.
    Seen,
    /// Acted on: the thing it pointed at was dealt with.
    Acted,
    /// Looked at and set aside on purpose.
    Dismissed,
}

impl AckAction {
    pub const ALL: [Self; 3] = [Self::Seen, Self::Acted, Self::Dismissed];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Seen => "seen",
            Self::Acted => "acted",
            Self::Dismissed => "dismissed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|action| action.as_str() == value)
    }
}

/// `notification.acknowledged`. `action` is optional and absent on every acknowledgement written
/// before it existed, which stay valid and replay unchanged (hivemind-m306.4.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationAcknowledgedPayload {
    pub notification_id: String,
    pub ack_at: DateTime<Utc>,
    pub snooze_until: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<AckAction>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceRecordedPayload {
    pub evidence_id: String,
    pub content: String,
    pub source: Option<String>,
}

/// What kind of premise a hypothesis is: a stated assumption, or an explicit bet — a
/// declared gap with nothing behind it yet. Neither ranks above the other; each gets its
/// own later question (assumption: did it hold? bet: did we check?). `#[serde(default)]`
/// on `HypothesisRecordedPayload::kind` makes `Assumption` the replay default for every
/// event recorded before this field existed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HypothesisKind {
    #[default]
    Assumption,
    Bet,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HypothesisRecordedPayload {
    pub hypothesis_id: String,
    pub statement: String,
    #[serde(default)]
    pub kind: HypothesisKind,
    /// When to check whether the bet paid off. Only meaningful for `kind: Bet`, but not
    /// rejected on an assumption — honesty about staleness is the renderer's job, not a
    /// write-time gate on what a caller chooses to track.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_by: Option<DateTime<Utc>>,
    /// What would change our mind, in the decider's own words. Set on a bet so the node
    /// reads standalone once the check date arrives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub would_change_if: Option<String>,
}

/// A question a decision answers, recorded once so every later decision that answers the same
/// question shares one node. `text` is the words as first written; the projector derives the
/// normalized form (`normalize_question_text`) the exact-match reuse compares on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionRecordedPayload {
    pub question_id: String,
    pub text: String,
}

/// An explicit ask, naming the question it targets (hivemind-bbnw.4). `text` is carried
/// alongside `question_id` (rather than requiring a join) for the same reason
/// `QuestionRecordedPayload` carries it: every event is self-sufficient for its own concern.
/// `asked_at` is the event's own `ts`, never a caller-supplied field — an ask is never
/// back-dated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionAskedPayload {
    pub question_id: String,
    pub text: String,
}

/// The form two spellings of one question are compared in: lowercase, whitespace collapsed to
/// single spaces, trailing sentence punctuation dropped. Exact match on this form, and nothing
/// fuzzier, is what lets two captures share a `Question` node: deterministic, no ranking, no
/// model (hivemind-zdsh.16). A text with no word in it normalizes to the empty string.
pub fn normalize_question_text(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed
        .to_lowercase()
        .trim_end_matches(|c: char| {
            c.is_whitespace() || matches!(c, '?' | '!' | '.' | ',' | ';' | ':' | '…')
        })
        .to_owned()
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
    /// A decision follows from a prior decision — a premise in the broad sense (refines it,
    /// answers a loose end it left open, is consistent with it), distinct from `Supersedes`
    /// (the parent still stands) and from evidence/assumption grounding (`BASED_ON`/`ASSUMES`,
    /// which name a claim about the world, not a decision already made).
    #[serde(rename = "FOLLOWS_FROM", alias = "follows_from")]
    FollowsFrom,
    /// A decision answers a question (`Decision` -> `Question`). A supersession chain is the same
    /// question answered again; two accepted answers that choose differently are a conflict.
    #[serde(rename = "ANSWERS", alias = "answers")]
    Answers,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IngestTurn {
    pub turn_id: String,
    pub role: String,
    pub text: String,
    pub truncated: bool,
    /// This turn's own time in the source transcript (Claude Code and Codex JSONL logs carry
    /// one per turn), not when hivemind received or classified the batch. `#[serde(default)]`
    /// so a caller that does not supply one — and every batch received before this field
    /// existed — still parses; classification then has no per-turn source time to prefer over
    /// the batch's own received/classified time, never a fabricated one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ts: Option<DateTime<Utc>>,
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
    /// ID of an already recorded decision this capture states again: the same choice, made or
    /// relayed once more. Only on a `decision`. The classifier judges it from the closest recorded
    /// decisions it was shown; the write path only checks that the id names a recorded decision.
    /// Recorded, it projects a `SAME_AS` link to that decision, so reads show one decision;
    /// restated from the very same moment (see `record_ingest_batch_classified`) it is not
    /// recorded a second time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restates_id: Option<String>,
    /// The `turn_id` of the turn, in one of the batches this classification covers, that this
    /// capture came from: the turn that made the decision, or the turn that asked the question
    /// of a `decision-request`. The classifier judges it; the write path only checks it names
    /// such a turn (refusing the classification otherwise) and reads that turn's own time into
    /// `source_ts`. Absent when the classifier could not tell which turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_turn_id: Option<String>,
    /// The time of the turn `source_turn_id` names (`IngestTurn::ts`), set by the write path from
    /// the received batch and never taken from the caller: whatever a submission carries here is
    /// replaced. Absent when the capture names no turn or that turn carries no time, in which
    /// case nothing is guessed and the capture reads as recorded at the classification's own
    /// time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_ts: Option<DateTime<Utc>>,
    /// The question, in the words it was asked in. On a `decision-request`: what is being
    /// asked. On a `decision`: the question it answers, when the text states one. Only on those
    /// two kinds, and only from the text. A decision's question is resolved to a `Question`
    /// node by `normalize_question_text`, exactly as `capture --question` does, so a decision
    /// and a request that state the same question share one node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
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
    /// The first (or only) batch id this classification covers. Kept for
    /// events written before session-grouped submission existed, and as a
    /// convenient single-batch accessor; `batch_ids` is authoritative when
    /// present. Always equals `batch_ids[0]` on events written by this
    /// version of the write path.
    pub batch_id: String,
    pub classifier_model: String,
    pub schema_version: String,
    pub captures: Vec<CaptureItem>,
    /// All batch ids this single classification call covers, in submission
    /// order. A session-grouped classification (hivemind-zdsh.18) covers
    /// more than one batch with one model call and one event, so every
    /// listed batch moves from pending to classified together. Empty on
    /// events written before this field existed — readers use `batch_id` as
    /// the fallback (see `classified_batch_ids`, which applies that same
    /// fallback over the raw event payload).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub batch_ids: Vec<String>,
}

/// The batch ids a raw `ingest.batch_classified` event payload covers: `batch_ids` when present
/// (session-grouped submissions), falling back to the singular `batch_id` of an event written
/// before `batch_ids` existed. Reads the raw payload so a scan never fails on one odd event.
pub fn classified_batch_ids(payload: &serde_json::Value) -> Vec<String> {
    let from_array: Vec<String> = payload
        .get("batch_ids")
        .and_then(|value| value.as_array())
        .map(|ids| {
            ids.iter()
                .filter_map(|id| id.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    if !from_array.is_empty() {
        return from_array;
    }
    payload
        .get("batch_id")
        .and_then(|value| value.as_str())
        .map(|id| vec![id.to_owned()])
        .unwrap_or_default()
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

/// Payload for a `decision.scored` append-only annotation event, **schema version 1**: the
/// float scores keyed by a classifier capture node. This is the shape every event written
/// before [`DecisionAssessedPayload`] has. The ledger is immutable, so these events keep parsing
/// and keep projecting as they always did; a payload without `schema_version` is this shape.
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

/// How much of a dimension a record supports. Ordinal, from published rules; not a fraction.
/// `None` means the record states nothing toward the dimension, not that the decision was bad.
///
/// It is wire vocabulary: the deterministic floors of the quality profile and a model's
/// assessment ([`ModelDimension`]) both speak it.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityLevel {
    None,
    Partial,
    Solid,
}

impl QualityLevel {
    /// The wire name (`partial`), the one serialization gives.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Partial => "partial",
            Self::Solid => "solid",
        }
    }
}

/// The `schema_version` a [`DecisionAssessedPayload`] carries. A `decision.scored` payload with
/// no `schema_version` is the version-1 shape ([`DecisionScoredPayload`]).
pub const DECISION_ASSESSED_SCHEMA_VERSION: u32 = 2;

/// A model's answer for one dimension: assessed, with the passage it rests on quoted, or not
/// assessed, with why. There is no third answer and never a placeholder score: a model that
/// cannot assess a dimension says so.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModelDimension {
    Assessed {
        level: QualityLevel,
        explanation: String,
        /// The passage of the decision's own recorded text the assessment rests on, verbatim.
        /// Required at level `partial` or `solid`; optional at level `none`, since a `none`
        /// judgement is usually about something the record lacks and an absence cannot be
        /// quoted (the explanation still says what is missing). When it is given it must be
        /// non-blank, and the write path refuses an event whose quote does not occur in the
        /// decision's text. Left out of the wire form when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        quote: Option<String>,
    },
    NotAssessed {
        reason: String,
    },
}

/// A model's answer for all seven dimensions. Every one is present: leaving a dimension out
/// is not a way to say it was not assessed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelDimensions {
    pub framing: ModelDimension,
    pub alternatives: ModelDimension,
    pub information: ModelDimension,
    pub reasoning: ModelDimension,
    pub values_tradeoffs: ModelDimension,
    pub bias_exposure: ModelDimension,
    pub calibration: ModelDimension,
}

impl ModelDimensions {
    /// The seven answers with their wire names, in the order `docs/DECISION_SCORING.md` lists
    /// the dimensions.
    pub fn entries(&self) -> [(&'static str, &ModelDimension); 7] {
        [
            ("framing", &self.framing),
            ("alternatives", &self.alternatives),
            ("information", &self.information),
            ("reasoning", &self.reasoning),
            ("values_tradeoffs", &self.values_tradeoffs),
            ("bias_exposure", &self.bias_exposure),
            ("calibration", &self.calibration),
        ]
    }
}

/// Payload for a `decision.scored` append-only annotation event, **schema version 2**: a
/// model's assessment of one decision, keyed by the decision's id (a proposed decision or a
/// classified capture, `capture:<event>:<index>`).
///
/// Each of the seven dimensions is either assessed (a level, an explanation and, at `partial`
/// or `solid`, the verbatim passage it rests on; a `none` answer may quote one) or not assessed
/// (and why). It is stored beside the deterministic floors of the quality profile and never
/// replaces them. Like a version-1 score it is Layer 3, append-only and never an edit to the
/// decision; the newest one for a decision is the one the graph shows and earlier ones stay in
/// the ledger. `supersedes_score_id` names the earlier event a re-assessment replaces (an audit
/// pointer, not enforced).
///
/// [`validate`] checks the shape. That every quote given occurs in the decision's recorded text
/// needs the ledger, so `Commands::record_decision_assessed` checks it before anything is
/// appended.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionAssessedPayload {
    /// Always [`DECISION_ASSESSED_SCHEMA_VERSION`].
    pub schema_version: u32,
    pub decision_id: String,
    /// The model that produced the assessment.
    pub model: String,
    /// The version of the prompt it was given, so an assessment can be traced to its wording.
    pub prompt_version: String,
    /// event_uuid of a prior `decision.scored` event that this assessment supersedes, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes_score_id: Option<String>,
    pub dimensions: ModelDimensions,
    /// Importance factors, a separate axis from the seven dimensions. Absent when the assessor
    /// did not judge them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub importance: Option<ImportanceFactors>,
}

/// What the graph keeps of a [`DecisionAssessedPayload`] on the decision node, as one JSON
/// property (`model_assessment`): who assessed, with which prompt, and the seven answers.
/// Importance is a separate axis and is stored as its own properties.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelAssessment {
    pub model: String,
    pub prompt_version: String,
    pub dimensions: ModelDimensions,
}

impl DecisionAssessedPayload {
    /// The part of this payload the graph keeps on the decision node.
    pub fn model_assessment(&self) -> ModelAssessment {
        ModelAssessment {
            model: self.model.clone(),
            prompt_version: self.prompt_version.clone(),
            dimensions: self.dimensions.clone(),
        }
    }

    /// The shape rules that need no ledger: the version, the names, a non-blank explanation for
    /// every assessed dimension and a non-blank quote for every one at level `partial` or
    /// `solid` (a `none` answer may leave its quote out; one it gives must be non-blank too), a
    /// non-blank reason for every dimension that is not assessed, and importance factors in
    /// range.
    pub fn validate_shape(&self) -> std::result::Result<(), EventValidationError> {
        if self.schema_version != DECISION_ASSESSED_SCHEMA_VERSION {
            return Err(EventValidationError::UnsupportedSchemaVersion {
                expected: DECISION_ASSESSED_SCHEMA_VERSION,
                got: self.schema_version,
            });
        }
        require_non_empty("payload.decision_id", &self.decision_id)?;
        require_non_empty("payload.model", &self.model)?;
        require_non_empty("payload.prompt_version", &self.prompt_version)?;
        require_optional_non_empty(
            "payload.supersedes_score_id",
            self.supersedes_score_id.as_deref(),
        )?;
        for (dimension, answer) in self.dimensions.entries() {
            match answer {
                ModelDimension::Assessed {
                    level,
                    explanation,
                    quote,
                } => {
                    require_dimension_text(dimension, "explanation", explanation)?;
                    match quote {
                        Some(quote) => require_dimension_text(dimension, "quote", quote)?,
                        None if *level == QualityLevel::None => {}
                        None => {
                            return Err(EventValidationError::QuoteRequired {
                                dimension,
                                level: level.as_str(),
                            });
                        }
                    }
                }
                ModelDimension::NotAssessed { reason } => {
                    require_dimension_text(dimension, "reason", reason)?;
                }
            }
        }
        if let Some(importance) = &self.importance {
            importance.validate_shape()?;
        }
        Ok(())
    }
}

impl ImportanceFactors {
    fn validate_shape(&self) -> std::result::Result<(), EventValidationError> {
        let unit = |value: f64| (0.0..=1.0).contains(&value);
        if !(self.stakes.is_finite() && self.stakes >= 0.0) {
            return Err(EventValidationError::InvalidImportance("stakes"));
        }
        if !unit(self.irreversibility) {
            return Err(EventValidationError::InvalidImportance("irreversibility"));
        }
        if !unit(self.actionability) {
            return Err(EventValidationError::InvalidImportance("actionability"));
        }
        require_non_empty(
            "payload.importance.stakes_explanation",
            &self.stakes_explanation,
        )?;
        require_non_empty(
            "payload.importance.irreversibility_explanation",
            &self.irreversibility_explanation,
        )?;
        require_non_empty(
            "payload.importance.actionability_explanation",
            &self.actionability_explanation,
        )
    }
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

/// How a decision's `project` field was determined (approved record shape, item 2).
/// Only `Stated` and `PersonalFallback` are produced by the write layer today
/// (hivemind-s15q.3); the rest are reserved for the surfaces that determine a project
/// from context — folder marker / rig (hivemind-s15q.12), current project
/// (hivemind-s15q.13), capture job (hivemind-s15q.15), and an explicit move
/// (hivemind-s15q.10) — so the wire format never needs to widen again as those land.
/// `Inherited` is recorded by a replacement that states no project and so takes the
/// project the decision it replaces is filed in (hivemind-9lzi).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectSource {
    Stated,
    FolderMarker,
    Rig,
    CurrentProject,
    Job,
    PersonalFallback,
    Moved,
    Inherited,
}

impl ProjectSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stated => "stated",
            Self::FolderMarker => "folder_marker",
            Self::Rig => "rig",
            Self::CurrentProject => "current_project",
            Self::Job => "job",
            Self::PersonalFallback => "personal_fallback",
            Self::Moved => "moved",
            Self::Inherited => "inherited",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "stated" => Some(Self::Stated),
            "folder_marker" => Some(Self::FolderMarker),
            "rig" => Some(Self::Rig),
            "current_project" => Some(Self::CurrentProject),
            "job" => Some(Self::Job),
            "personal_fallback" => Some(Self::PersonalFallback),
            "moved" => Some(Self::Moved),
            "inherited" => Some(Self::Inherited),
            _ => None,
        }
    }
}

/// `decision.moved` (approved record shape, item 3). `from` must equal the decision's
/// project at the time of the move and `to` must differ — enforced by
/// `commands::move_decision`, not here; the event schema only shapes the wire format.
/// Reversal is another `DecisionMovedPayload` with `from`/`to` swapped; nothing here is
/// ever edited in place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionMovedPayload {
    pub decision_id: String,
    pub from: String,
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// `decision.retitled` (hivemind-ydmp). `from` is the current title at retitle time and is
/// never validated here — it may legitimately be over the title cap, since retiring those
/// pre-cap titles is the whole point. `to` must pass `validate_title` — enforced by
/// `commands::retitle_decision`, not here. Reversal is another `DecisionRetitledPayload`
/// with `from`/`to` swapped; nothing here is ever edited in place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionRetitledPayload {
    pub decision_id: String,
    pub from: String,
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
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

/// `project.topic_declared`: `topic_key` joined `handle`'s vocabulary. The key is already
/// normalised (lowercase kebab, see `commands::normalize_topic_key`) when the write layer
/// records it. Declaring is the only way a vocabulary grows; there is no retraction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectTopicDeclaredPayload {
    pub handle: String,
    pub topic_key: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EventPayload {
    DecisionProposed(DecisionProposedPayload),
    DecisionRequested(DecisionRequestedPayload),
    DecisionAccepted(DecisionAcceptedPayload),
    DecisionRejected(DecisionRejectedPayload),
    DecisionSuperseded(DecisionSupersededPayload),
    EvidenceRecorded(EvidenceRecordedPayload),
    HypothesisRecorded(HypothesisRecordedPayload),
    QuestionRecorded(QuestionRecordedPayload),
    QuestionAsked(QuestionAskedPayload),
    RelationAdded(RelationAddedPayload),
    RelationRemoved(RelationRemovedPayload),
    BlockerReported(BlockerReportedPayload),
    BlockerResolved(BlockerResolvedPayload),
    NotificationSent(NotificationSentPayload),
    NotificationAcknowledged(NotificationAcknowledgedPayload),
    SuggestionSurfaced(SuggestionSurfacedPayload),
    IngestBatchReceived(IngestBatchReceivedPayload),
    IngestBatchClassified(IngestBatchClassifiedPayload),
    DecisionScored(DecisionScoredPayload),
    /// A `decision.scored` event of schema version 2 (see [`DecisionAssessedPayload`]).
    DecisionAssessed(DecisionAssessedPayload),
    DecisionMetadataDerived(DecisionMetadataDerivedPayload),
    DecisionMoved(DecisionMovedPayload),
    DecisionRetitled(DecisionRetitledPayload),
    ProjectRegistered(ProjectRegisteredPayload),
    ProjectLinked(ProjectLinkPayload),
    ProjectUnlinked(ProjectLinkPayload),
    ProjectAnchored(ProjectAnchorPayload),
    ProjectUnanchored(ProjectAnchorPayload),
    ProjectTopicDeclared(ProjectTopicDeclaredPayload),
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
            Self::QuestionRecorded(_) => EventType::QuestionRecorded,
            Self::QuestionAsked(_) => EventType::QuestionAsked,
            Self::RelationAdded(_) => EventType::RelationAdded,
            Self::RelationRemoved(_) => EventType::RelationRemoved,
            Self::BlockerReported(_) => EventType::BlockerReported,
            Self::BlockerResolved(_) => EventType::BlockerResolved,
            Self::NotificationSent(_) => EventType::NotificationSent,
            Self::NotificationAcknowledged(_) => EventType::NotificationAcknowledged,
            Self::SuggestionSurfaced(_) => EventType::SuggestionSurfaced,
            Self::IngestBatchReceived(_) => EventType::IngestBatchReceived,
            Self::IngestBatchClassified(_) => EventType::IngestBatchClassified,
            Self::DecisionScored(_) | Self::DecisionAssessed(_) => EventType::DecisionScored,
            Self::DecisionMetadataDerived(_) => EventType::DecisionMetadataDerived,
            Self::DecisionMoved(_) => EventType::DecisionMoved,
            Self::DecisionRetitled(_) => EventType::DecisionRetitled,
            Self::ProjectRegistered(_) => EventType::ProjectRegistered,
            Self::ProjectLinked(_) => EventType::ProjectLinked,
            Self::ProjectUnlinked(_) => EventType::ProjectUnlinked,
            Self::ProjectAnchored(_) => EventType::ProjectAnchored,
            Self::ProjectUnanchored(_) => EventType::ProjectUnanchored,
            Self::ProjectTopicDeclared(_) => EventType::ProjectTopicDeclared,
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
            Self::QuestionRecorded(payload) => serde_json::to_value(payload),
            Self::QuestionAsked(payload) => serde_json::to_value(payload),
            Self::RelationAdded(payload) => serde_json::to_value(payload),
            Self::RelationRemoved(payload) => serde_json::to_value(payload),
            Self::BlockerReported(payload) => serde_json::to_value(payload),
            Self::BlockerResolved(payload) => serde_json::to_value(payload),
            Self::NotificationSent(payload) => serde_json::to_value(payload),
            Self::NotificationAcknowledged(payload) => serde_json::to_value(payload),
            Self::SuggestionSurfaced(payload) => serde_json::to_value(payload),
            Self::IngestBatchReceived(payload) => serde_json::to_value(payload),
            Self::IngestBatchClassified(payload) => serde_json::to_value(payload),
            Self::DecisionScored(payload) => serde_json::to_value(payload),
            Self::DecisionAssessed(payload) => serde_json::to_value(payload),
            Self::DecisionMetadataDerived(payload) => serde_json::to_value(payload),
            Self::DecisionMoved(payload) => serde_json::to_value(payload),
            Self::DecisionRetitled(payload) => serde_json::to_value(payload),
            Self::ProjectRegistered(payload) => serde_json::to_value(payload),
            Self::ProjectLinked(payload) => serde_json::to_value(payload),
            Self::ProjectUnlinked(payload) => serde_json::to_value(payload),
            Self::ProjectAnchored(payload) => serde_json::to_value(payload),
            Self::ProjectUnanchored(payload) => serde_json::to_value(payload),
            Self::ProjectTopicDeclared(payload) => serde_json::to_value(payload),
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

    #[error("{0} requires {1} — a verbatim quote with no stated question is unreadable once the source conversation is gone")]
    RequiresPairedField(&'static str, &'static str),

    #[error("payload.delegated_by must name a human actor (human:<name>), got {0:?}")]
    DelegatedByNotHuman(String),

    #[error("payload.delegated_by is only valid on an agent's own acceptance: accepting actor {0:?} is not an agent (agent:<tool>:<name>)")]
    DelegationRequiresAgentAccepter(String),

    #[error("payload.schema_version must be {expected}, got {got}")]
    UnsupportedSchemaVersion { expected: u32, got: u32 },

    #[error("payload.dimensions.{dimension}.{field} must not be empty")]
    EmptyDimensionField {
        dimension: &'static str,
        field: &'static str,
    },

    #[error("payload.dimensions.{dimension}.quote is required at level {level}: a partial or solid assessment must quote the passage of the decision's own text it rests on (only a none answer may leave the quote out)")]
    QuoteRequired {
        dimension: &'static str,
        level: &'static str,
    },

    #[error("payload.importance.{0} is out of range")]
    InvalidImportance(&'static str),

    #[error("payload does not match event type {event_type:?}: {source}")]
    Payload {
        event_type: EventType,
        #[source]
        source: serde_json::Error,
    },
}

/// An annotation row a reader could not make sense of: a `decision.scored` (either shape) or
/// `decision.metadata_derived` event whose payload fails [`validate`]. These are Layer-3
/// enrichment of a decision, never part of what was decided, and the ledger is append-only and
/// shared, so a buggy or old producer, a partial import or a hand edit can leave one behind. A
/// reader treats it as "annotation unavailable" — skips it, keeps everything the decision's own
/// events say — and says so; it never fails the read over it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnreadableAnnotation {
    pub event_id: Option<EventId>,
    pub event_type: EventType,
    /// Why [`validate`] refused the row.
    pub reason: String,
}

/// What a reader gets from one ledger row: its payload, or, for an annotation row that cannot
/// be read, the fact that it was skipped. Every other event kind that fails validation is still
/// an error: those rows are what the graph is built from.
#[derive(Debug, Clone, PartialEq)]
pub enum ReadEvent {
    Payload(Box<EventPayload>),
    Unreadable(UnreadableAnnotation),
}

impl EventType {
    /// Layer-3 enrichment events: an assessment or score of a decision, or metadata derived
    /// from it. A reader may skip an unreadable one ([`validate_for_read`]).
    pub fn is_annotation(self) -> bool {
        matches!(self, Self::DecisionScored | Self::DecisionMetadataDerived)
    }
}

/// [`validate`] for a reader (the projector and every query over the ledger). The write path
/// keeps using [`validate`], which refuses a malformed annotation before it is appended.
pub fn validate_for_read(event: &Event) -> std::result::Result<ReadEvent, EventValidationError> {
    match validate(event) {
        Ok(payload) => Ok(ReadEvent::Payload(Box::new(payload))),
        Err(error) if event.event_type.is_annotation() => {
            Ok(ReadEvent::Unreadable(UnreadableAnnotation {
                event_id: event.event_id,
                event_type: event.event_type,
                reason: error.to_string(),
            }))
        }
        Err(error) => Err(error),
    }
}

/// True when `event` is an annotation row that [`validate_for_read`] would skip.
pub fn is_unreadable_annotation(event: &Event) -> bool {
    event.event_type.is_annotation() && validate(event).is_err()
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
            // A quote needs the question it answers; a question stands alone (it names the
            // `Question` node the decision answers, hivemind-zdsh.16).
            if payload.quote.is_some() && payload.question.is_none() {
                return Err(EventValidationError::RequiresPairedField(
                    "payload.quote",
                    "payload.question",
                ));
            }
            require_optional_non_empty("payload.project", payload.project.as_deref())?;
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
            let payload: DecisionAcceptedPayload = parse_payload(event)?;
            require_non_empty("payload.decision_id", &payload.decision_id)?;
            require_optional_non_empty("payload.delegated_by", payload.delegated_by.as_deref())?;
            if let Some(delegated_by) = payload.delegated_by.as_deref() {
                require_delegation_shape(&event.actor_id, delegated_by)?;
            }
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
            require_optional_non_empty(
                "payload.would_change_if",
                payload.would_change_if.as_deref(),
            )?;
            Ok(EventPayload::HypothesisRecorded(payload))
        }
        EventType::QuestionRecorded => {
            let payload: QuestionRecordedPayload = parse_payload(event)?;
            require_non_empty("payload.question_id", &payload.question_id)?;
            require_non_empty("payload.text", &payload.text)?;
            if normalize_question_text(&payload.text).is_empty() {
                return Err(EventValidationError::EmptyField("payload.text"));
            }
            Ok(EventPayload::QuestionRecorded(payload))
        }
        EventType::QuestionAsked => {
            let payload: QuestionAskedPayload = parse_payload(event)?;
            require_non_empty("payload.question_id", &payload.question_id)?;
            require_non_empty("payload.text", &payload.text)?;
            if normalize_question_text(&payload.text).is_empty() {
                return Err(EventValidationError::EmptyField("payload.text"));
            }
            Ok(EventPayload::QuestionAsked(payload))
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
        EventType::SuggestionSurfaced => {
            require_event_provenance(event)?;
            let payload: SuggestionSurfacedPayload = parse_payload(event)?;
            require_non_empty("payload.finding_id", &payload.finding_id)?;
            require_non_empty("payload.decision_id", &payload.decision_id)?;
            require_non_empty("payload.recipient_actor_id", &payload.recipient_actor_id)?;
            require_non_empty("payload.channel", &payload.channel)?;
            Ok(EventPayload::SuggestionSurfaced(payload))
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
        // A payload with a `schema_version` is the model-assessment shape; every event written
        // before it has none and stays the version-1 shape below.
        EventType::DecisionScored if event.payload.get("schema_version").is_some() => {
            let payload: DecisionAssessedPayload = parse_payload(event)?;
            payload.validate_shape()?;
            Ok(EventPayload::DecisionAssessed(payload))
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
        EventType::DecisionMoved => {
            let payload: DecisionMovedPayload = parse_payload(event)?;
            require_non_empty("payload.decision_id", &payload.decision_id)?;
            require_non_empty("payload.from", &payload.from)?;
            require_non_empty("payload.to", &payload.to)?;
            require_optional_non_empty("payload.reason", payload.reason.as_deref())?;
            Ok(EventPayload::DecisionMoved(payload))
        }
        EventType::DecisionRetitled => {
            let payload: DecisionRetitledPayload = parse_payload(event)?;
            require_non_empty("payload.decision_id", &payload.decision_id)?;
            require_non_empty("payload.from", &payload.from)?;
            require_non_empty("payload.to", &payload.to)?;
            require_optional_non_empty("payload.reason", payload.reason.as_deref())?;
            Ok(EventPayload::DecisionRetitled(payload))
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
        EventType::ProjectTopicDeclared => {
            let payload: ProjectTopicDeclaredPayload = parse_payload(event)?;
            require_non_empty("payload.handle", &payload.handle)?;
            require_non_empty("payload.topic_key", &payload.topic_key)?;
            Ok(EventPayload::ProjectTopicDeclared(payload))
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

fn require_dimension_text(
    dimension: &'static str,
    field: &'static str,
    value: &str,
) -> std::result::Result<(), EventValidationError> {
    if value.trim().is_empty() {
        Err(EventValidationError::EmptyDimensionField { dimension, field })
    } else {
        Ok(())
    }
}

/// Shape half of the delegation-marker rules (hivemind-zdsh.6), the part decidable from the
/// event alone: the delegator is a human actor and the accepter is an agent. That the
/// accepter is also the decision's proposer needs ledger state and is enforced by
/// `Commands::accept_decision_delegated`.
pub(crate) fn require_delegation_shape(
    accepter_id: &str,
    delegated_by: &str,
) -> std::result::Result<(), EventValidationError> {
    let names_human = delegated_by
        .strip_prefix("human:")
        .is_some_and(|name| !name.trim().is_empty());
    if !names_human {
        return Err(EventValidationError::DelegatedByNotHuman(
            delegated_by.to_owned(),
        ));
    }
    if !accepter_id.starts_with("agent:") {
        return Err(EventValidationError::DelegationRequiresAgentAccepter(
            accepter_id.to_owned(),
        ));
    }
    Ok(())
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
