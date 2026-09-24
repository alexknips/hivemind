//! Write-layer commands: validate invariants and append events to the ledger; the sole entry point for all mutations.
//!
//! # Grounding invariants (what a decision rests on)
//!
//! Enforced by `propose_decision` / `propose_decision_with_id` (at capture) and
//! `ground_decision` (later); each has a test in `commands/tests.rs`:
//!
//! - `Grounding::Declared` with no premise decision, evidence item or hypothesis is a
//!   `Validation` error — a bet counts, because it is a hypothesis of kind `bet`.
//!   `Grounding::NotAsked` (raw emit, classifier ingest, document import, Slack) is never
//!   refused.
//! - Every premise decision id must exist in the tenant and must not be the decision being
//!   proposed or grounded.
//! - A superseded or rejected premise is allowed: the ledger is append-only, the link is
//!   recorded, and the caller is told via `DecisionProposalEventIds::premise_stale`.
//! - At capture, one `FOLLOWS_FROM` (decision -> premise) per premise is fanned out with
//!   `causation_event_id` = the proposal event. `ground_decision` appends `FOLLOWS_FROM` /
//!   `BASED_ON` / `ASSUMES` with no causation, which is how "at capture" and "added later"
//!   stay distinguishable without a new field.
//! - `expressed_confidence`, when present, is `low`, `medium` or `high`.
//! - `record_bet` with no statement records `Judgement call: <decision title>`;
//!   `would_change_if`, when present, is non-empty.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::{Mutex, MutexGuard};

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::error::CommandError;
use crate::events::{
    CaptureItem, DecisionIdPayload, DecisionProposedPayload, DecisionRejectedPayload,
    DecisionScoredPayload, DecisionSupersededPayload, Event, EventBuilder, EventId, EventPayload,
    EventProvenance, EventType, EvidenceRecordedPayload, HypothesisKind, HypothesisRecordedPayload,
    IngestBatchClassifiedPayload, IngestBatchReceivedPayload, IngestTurn, ProjectAnchorKind,
    ProjectAnchorPayload, ProjectLinkKind, ProjectLinkPayload, ProjectRegisteredPayload,
    ProjectSource, RelationAddedPayload, RelationKind, TenantId,
};
use crate::ledger::EventLedger;
use crate::util::{require_non_empty, require_valid_actor_id};
use crate::Result;

pub type DecisionId = String;
pub type EvidenceId = String;
pub type HypothesisId = String;
pub type OptionId = String;

pub const MAX_TOPIC_KEY_LEN: usize = 64;
pub const MIN_PROJECT_HANDLE_LEN: usize = 2;
pub const MAX_PROJECT_HANDLE_LEN: usize = 40;
pub const PERSONAL_PROJECT_HANDLE_PREFIX: &str = "personal:";
/// Appended to every capture reply whose project was `personal_fallback`, so a decision that
/// landed in the recorder's own project is never silent (Alex's choice 3a). One sentence,
/// shared by every surface (CLI text, CLI JSON, MCP) so they can't drift.
pub const PERSONAL_FALLBACK_NOTICE: &str =
    "saved to your personal project; pass a registered project handle to file it under a shared one";
/// A title is a name, not a summary: one short sentence a reader can scan in a list.
/// Longer reasoning belongs in `rationale`, which has no such cap.
pub const MAX_TITLE_LEN: usize = 120;
/// Minimum trimmed length for `rationale` — see `require_readable_rationale`.
pub const MIN_RATIONALE_CHARS: usize = 20;
/// Minimum whitespace-separated word count for `rationale` — see `require_readable_rationale`.
pub const MIN_RATIONALE_WORDS: usize = 4;

#[derive(Debug, Clone)]
pub struct DecisionProposalEventUuids {
    pub proposal: Uuid,
    pub has_option: Vec<Uuid>,
    pub chose: Option<Uuid>,
    pub assumes: Vec<Uuid>,
    pub based_on: Vec<Uuid>,
    pub follows_from: Vec<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionProposalEventIds {
    pub proposal_event_id: EventId,
    pub relation_event_ids: Vec<EventId>,
    /// Premise decision ids named in `Grounding::Declared` that were already superseded or
    /// rejected at capture time. Append-only: the premise link is still recorded (honesty
    /// about staleness is the renderer's job, not a write-time gate), but the caller needs
    /// this to build an honest reply.
    pub premise_stale: Vec<DecisionId>,
}

/// What a proposed decision rests on, named at the moment of capture. `Declared` requires
/// at least one id across the three lists — a bet is declared separately as a hypothesis
/// of `HypothesisKind::Bet` in `hypothesis_ids`, so an all-empty `Declared` really does mean
/// nothing was named. `NotAsked` is reserved for paths that never ask the question: raw
/// `emit decision.proposed`, `ingest.batch_classified`, document import, and Slack capture.
/// Grounding is a write-time validation gate only — it is never itself persisted; the graph
/// already derives "grounded" from the FOLLOWS_FROM/BASED_ON/ASSUMES edges it produces.
#[derive(Debug, Clone, Copy)]
pub enum Grounding<'a> {
    Declared {
        premise_decision_ids: &'a [String],
        evidence_ids: &'a [String],
        hypothesis_ids: &'a [String],
    },
    NotAsked,
}

impl<'a> Grounding<'a> {
    fn premise_decision_ids(self) -> &'a [String] {
        match self {
            Self::Declared {
                premise_decision_ids,
                ..
            } => premise_decision_ids,
            Self::NotAsked => &[],
        }
    }

    /// True only for `Declared` with nothing named across all three lists — the one shape
    /// `propose_decision`/`propose_decision_with_id` refuse. `NotAsked` is never empty in
    /// this sense: the question was never posed, so there is nothing to be empty about.
    fn declared_and_empty(&self) -> bool {
        match self {
            Self::Declared {
                premise_decision_ids,
                evidence_ids,
                hypothesis_ids,
            } => {
                premise_decision_ids.is_empty()
                    && evidence_ids.is_empty()
                    && hypothesis_ids.is_empty()
            }
            Self::NotAsked => false,
        }
    }
}

/// A project the caller determined for a capture, plus how it got there. The write layer
/// validates the address (registered, not a reserved `personal:` handle) and the source
/// (see `Commands::resolve_stated_project`); it never infers a project itself -- working
/// out "which project" is agent-side (three-layer rule).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeterminedProject<'a> {
    pub handle: &'a str,
    pub source: ProjectSource,
}

impl<'a> DeterminedProject<'a> {
    /// The caller named the handle outright (`--project`, or the MCP `project` argument).
    pub const fn stated(handle: &'a str) -> Self {
        Self {
            handle,
            source: ProjectSource::Stated,
        }
    }
}

/// Where a decision was filed and how that was determined, as the write layer recorded it.
/// Returned to the caller so every capture reply can name its project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DecisionPlacement {
    /// The shared handle, or on personal fallback the derived `personal:<actor>` address.
    pub project: String,
    pub project_source: ProjectSource,
}

impl DecisionPlacement {
    /// The placement the projector derives from a recorded `project`/`project_source` pair
    /// (`None`/`None` is an event predating both fields: personal fallback).
    fn from_recorded(
        actor_id: &str,
        project: Option<&str>,
        project_source: Option<ProjectSource>,
    ) -> Self {
        Self {
            project: project.map_or_else(|| personal_project_handle(actor_id), ToOwned::to_owned),
            project_source: project_source.unwrap_or(ProjectSource::PersonalFallback),
        }
    }

    /// The "saved to your personal project" sentence when the write layer fell back to the
    /// recorder's personal project, `None` when a project was determined.
    pub fn notice(&self) -> Option<&'static str> {
        (self.project_source == ProjectSource::PersonalFallback).then_some(PERSONAL_FALLBACK_NOTICE)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupersedeOutcome {
    pub new_decision_id: DecisionId,
    pub proposal_event_id: EventId,
    pub relation_event_ids: Vec<EventId>,
    pub superseded_event_id: EventId,
    pub placement: DecisionPlacement,
}

#[derive(Debug, Clone, Copy)]
pub struct DecisionProposalInput<'a> {
    pub actor_id: &'a str,
    pub title: &'a str,
    pub rationale: &'a str,
    pub topic_keys: &'a [String],
    pub option_ids: &'a [String],
    /// Human-readable label per `option_ids` entry, index-aligned. Empty is accepted (labels
    /// unknown to the caller); a non-empty slice shorter than `option_ids` falls back to the
    /// raw id for the missing tail when rendered.
    pub option_labels: &'a [String],
    pub chosen_option_id: Option<&'a str>,
    /// Actor who actually made the decision, when it differs from `actor_id` (the recording
    /// actor/scribe) — e.g. an agent scribing a decision a specific human made. Requires
    /// `chosen_option_id` to be `Some`. Mutually exclusive with `still_proposed`.
    pub decided_by: Option<&'a str>,
    /// Keep the decision at `proposed` even though `chosen_option_id` is set, for a genuine
    /// open recommendation awaiting someone else's decision. Defaults to `false`: per
    /// hivemind-zdsh.8, a `chosen_option_id` means the decision was already made, so
    /// `propose_decision` auto-accepts it immediately after proposing — from `decided_by`
    /// when given, otherwise self-accepted from `actor_id` — unless this is `true`.
    pub still_proposed: bool,
    pub hypothesis_ids: &'a [String],
    pub evidence_ids: &'a [String],
    /// Verbatim words of the decider, self-contained. Requires `question` — see
    /// `propose_decision`.
    pub quote: Option<&'a str>,
    /// The question `quote` answers, spelled out. Requires `quote`.
    pub question: Option<&'a str>,
    /// What this decision rests on. See `Grounding`.
    pub grounding: Grounding<'a>,
    /// Expressed confidence from the decider's own words: low | medium | high. Never
    /// system-computed. Validated against that fixed vocabulary when present.
    pub expressed_confidence: Option<&'a str>,
    /// Registered project to file this decision under, with how the caller determined it.
    /// `None` means no handle was given: the write layer records
    /// `project_source = personal_fallback` and the projector derives the recorder's
    /// personal project from `actor_id`. A handle that isn't registered is refused (see
    /// `Commands::resolve_stated_project`).
    pub project: Option<DeterminedProject<'a>>,
}

#[derive(Debug, Clone, Copy)]
pub struct SupersedeInput<'a> {
    pub actor_id: &'a str,
    pub old_decision_id: &'a str,
    pub new_title: &'a str,
    pub new_rationale: &'a str,
    pub topic_keys: &'a [String],
    pub option_labels: &'a [String],
    pub chosen_option_label: Option<&'a str>,
    pub hypothesis_ids: &'a [String],
    pub evidence_ids: &'a [String],
    /// Explicit project override for the superseding decision. `None` means "not
    /// stated": the new decision inherits the old decision's `project` and
    /// `project_source` verbatim rather than defaulting to personal fallback.
    pub project: Option<DeterminedProject<'a>>,
}

/// Input to `Commands::ground_decision`: give an already-proposed decision its grounding
/// after the fact. Unlike `Grounding::Declared` on `DecisionProposalInput`, this is not
/// mutually exclusive with `NotAsked` — grounding an existing decision is always a
/// deliberate act, so there is nothing to make optional.
#[derive(Debug, Clone, Copy)]
pub struct GroundInput<'a> {
    pub actor_id: &'a str,
    pub decision_id: &'a str,
    pub premise_decision_ids: &'a [String],
    pub evidence_ids: &'a [String],
    pub hypothesis_ids: &'a [String],
}

pub struct Commands<'a, L: EventLedger> {
    ledger: &'a L,
    context: CommandContext,
    state: Mutex<CommandState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandContext {
    pub tenant_id: TenantId,
    pub provenance: EventProvenance,
}

impl CommandContext {
    pub fn new(tenant_id: TenantId, provenance: EventProvenance) -> Self {
        Self {
            tenant_id,
            provenance,
        }
    }

    pub fn local(provenance: EventProvenance) -> Self {
        Self::new(TenantId::local(), provenance)
    }
}

#[derive(Default)]
struct CommandState {
    option_ids: HashSet<OptionId>,
    /// Description captured alongside each option's label at `record_option_with_id` time.
    /// `propose_decision_with_id` reads this back to persist it onto the `DecisionProposed`
    /// event — the only place a description is durably stored (see hivemind-zdsh.10).
    option_descriptions: HashMap<OptionId, String>,
}

#[derive(Debug, Clone)]
struct DecisionProposalSnapshot {
    event_id: EventId,
    decision_id: DecisionId,
    actor_id: String,
    title: String,
    rationale: String,
    topic_keys: Vec<String>,
    option_ids: Vec<String>,
    chosen_option_id: Option<String>,
    hypothesis_ids: Vec<String>,
    evidence_ids: Vec<String>,
    project: Option<String>,
    project_source: Option<ProjectSource>,
}

impl<'a, L: EventLedger> Commands<'a, L> {
    pub fn new(ledger: &'a L) -> Self {
        Self::new_with_provenance(ledger, EventProvenance::cli())
    }

    pub fn new_with_provenance(ledger: &'a L, provenance: EventProvenance) -> Self {
        Self::new_with_context(ledger, CommandContext::local(provenance))
    }

    pub fn new_with_context(ledger: &'a L, context: CommandContext) -> Self {
        Self {
            ledger,
            context,
            state: Mutex::new(CommandState::default()),
        }
    }

    pub fn record_evidence(&self, actor_id: &str, content: &str) -> Result<EvidenceId> {
        require_valid_actor_id(actor_id)?;
        require_non_empty("content", content)?;

        let evidence_id = generate_entity_id("evidence");
        self.record_evidence_with_id(actor_id, &evidence_id, content, None, Uuid::new_v4())?;
        Ok(evidence_id)
    }

    pub fn record_evidence_with_id(
        &self,
        actor_id: &str,
        evidence_id: &str,
        content: &str,
        source: Option<&str>,
        event_uuid: Uuid,
    ) -> Result<EventId> {
        require_valid_actor_id(actor_id)?;
        require_non_empty("evidence_id", evidence_id)?;
        require_non_empty("content", content)?;
        require_optional_non_empty("source", source)?;

        let event = self.event_with_uuid(
            actor_id,
            EventPayload::EvidenceRecorded(EvidenceRecordedPayload {
                evidence_id: evidence_id.to_owned(),
                content: content.to_owned(),
                source: source.map(ToOwned::to_owned),
            }),
            None,
            event_uuid,
        )?;

        self.append_event(event)
    }

    pub fn record_hypothesis(&self, actor_id: &str, statement: &str) -> Result<HypothesisId> {
        require_valid_actor_id(actor_id)?;
        require_non_empty("statement", statement)?;

        let hypothesis_id = generate_entity_id("hypothesis");
        self.record_hypothesis_with_id(
            actor_id,
            &hypothesis_id,
            statement,
            HypothesisKind::Assumption,
            None,
            None,
            Uuid::new_v4(),
        )?;
        Ok(hypothesis_id)
    }

    /// Like `record_hypothesis`, but for a caller (e.g. `emit hypothesis.recorded --kind
    /// bet`) that names the full grounding shape up front instead of taking the assumption
    /// default.
    pub fn record_hypothesis_with_kind(
        &self,
        actor_id: &str,
        statement: &str,
        kind: HypothesisKind,
        check_by: Option<DateTime<Utc>>,
        would_change_if: Option<&str>,
    ) -> Result<HypothesisId> {
        require_valid_actor_id(actor_id)?;
        require_non_empty("statement", statement)?;

        let hypothesis_id = generate_entity_id("hypothesis");
        self.record_hypothesis_with_id(
            actor_id,
            &hypothesis_id,
            statement,
            kind,
            check_by,
            would_change_if,
            Uuid::new_v4(),
        )?;
        Ok(hypothesis_id)
    }

    /// Record a bet: a hypothesis of `HypothesisKind::Bet`, the "nothing yet, a declared gap"
    /// answer to "what does this rest on?". A bare bet (`statement` absent or blank) records
    /// `Judgement call: <decision title>` so the node reads standalone — that default is why
    /// the caller passes the title of the decision the bet grounds. The title is only read,
    /// and only required, when the statement is defaulted.
    pub fn record_bet(
        &self,
        actor_id: &str,
        statement: Option<&str>,
        decision_title: &str,
        check_by: Option<DateTime<Utc>>,
        would_change_if: Option<&str>,
    ) -> Result<HypothesisId> {
        let default_statement;
        let statement = match statement {
            Some(text) if !text.trim().is_empty() => text,
            _ => {
                require_non_empty("decision_title", decision_title)?;
                default_statement = format!("Judgement call: {}", decision_title.trim());
                default_statement.as_str()
            }
        };

        self.record_hypothesis_with_kind(
            actor_id,
            statement,
            HypothesisKind::Bet,
            check_by,
            would_change_if,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_hypothesis_with_id(
        &self,
        actor_id: &str,
        hypothesis_id: &str,
        statement: &str,
        kind: HypothesisKind,
        check_by: Option<DateTime<Utc>>,
        would_change_if: Option<&str>,
        event_uuid: Uuid,
    ) -> Result<EventId> {
        require_valid_actor_id(actor_id)?;
        require_non_empty("hypothesis_id", hypothesis_id)?;
        require_non_empty("statement", statement)?;
        require_optional_non_empty("would_change_if", would_change_if)?;

        let event = self.event_with_uuid(
            actor_id,
            EventPayload::HypothesisRecorded(HypothesisRecordedPayload {
                hypothesis_id: hypothesis_id.to_owned(),
                statement: statement.to_owned(),
                kind,
                check_by,
                would_change_if: would_change_if.map(ToOwned::to_owned),
            }),
            None,
            event_uuid,
        )?;

        self.append_event(event)
    }

    /// Register a shared project. Personal projects (handles under the reserved
    /// `personal:` prefix) are derived from the actor, never registered — see
    /// `validate_project_handle`.
    pub fn register_project(
        &self,
        actor_id: &str,
        handle: &str,
        display_name: Option<&str>,
        purpose: Option<&str>,
    ) -> Result<EventId> {
        require_non_empty("actor_id", actor_id)?;
        validate_project_handle(handle)?;
        require_optional_non_empty("display_name", display_name)?;
        require_optional_non_empty("purpose", purpose)?;

        if let Some(existing) = self.find_project_registered(handle)? {
            let existing_name = existing.display_name.unwrap_or(existing.handle);
            return Err(CommandError::Invariant(format!(
                "project handle already registered: {handle} (existing project: {existing_name})"
            ))
            .into());
        }

        let event = self.event_with_uuid(
            actor_id,
            EventPayload::ProjectRegistered(ProjectRegisteredPayload {
                handle: handle.to_owned(),
                display_name: display_name.map(ToOwned::to_owned),
                purpose: purpose.map(ToOwned::to_owned),
            }),
            None,
            Uuid::new_v4(),
        )?;

        self.append_event(event)
    }

    /// Link two registered projects. `part_of` allows at most one active parent per
    /// project (a tree of any depth); `depends_on` has no such limit.
    pub fn link_project(
        &self,
        actor_id: &str,
        from: &str,
        to: &str,
        kind: ProjectLinkKind,
    ) -> Result<EventId> {
        require_non_empty("actor_id", actor_id)?;
        require_non_empty("from", from)?;
        require_non_empty("to", to)?;

        if same_identifier(from, to) {
            return Err(
                CommandError::Validation("a project cannot link to itself".to_owned()).into(),
            );
        }
        if !self.project_exists(from)? {
            return Err(CommandError::Invariant(format!("project does not exist: {from}")).into());
        }
        if !self.project_exists(to)? {
            return Err(CommandError::Invariant(format!("project does not exist: {to}")).into());
        }
        if kind == ProjectLinkKind::PartOf {
            if let Some(existing_parent) = self.project_part_of_parent(from)? {
                if !same_identifier(&existing_parent, to) {
                    return Err(CommandError::Invariant(format!(
                        "project {from} already has a part_of parent: {existing_parent}"
                    ))
                    .into());
                }
            }
        }

        let event = self.event_with_uuid(
            actor_id,
            EventPayload::ProjectLinked(ProjectLinkPayload {
                from: from.to_owned(),
                to: to.to_owned(),
                kind,
            }),
            None,
            Uuid::new_v4(),
        )?;

        self.append_event(event)
    }

    /// Remove a currently active link between two projects. Refused when no such link
    /// is active — an unlink never silently creates the fact it claims to be retracting.
    pub fn unlink_project(
        &self,
        actor_id: &str,
        from: &str,
        to: &str,
        kind: ProjectLinkKind,
    ) -> Result<EventId> {
        require_non_empty("actor_id", actor_id)?;
        require_non_empty("from", from)?;
        require_non_empty("to", to)?;

        if !self.project_link_exists(from, to, kind)? {
            return Err(CommandError::Invariant(format!(
                "no active {} link from {from} to {to}",
                kind.as_str()
            ))
            .into());
        }

        let event = self.event_with_uuid(
            actor_id,
            EventPayload::ProjectUnlinked(ProjectLinkPayload {
                from: from.to_owned(),
                to: to.to_owned(),
                kind,
            }),
            None,
            Uuid::new_v4(),
        )?;

        self.append_event(event)
    }

    /// Anchor a registered project to a place in the world. Rig anchor values are unique
    /// per tenant — folder-marker overlap is enforced by the checked-in marker file, not
    /// centrally here.
    pub fn anchor_project(
        &self,
        actor_id: &str,
        handle: &str,
        anchor_kind: ProjectAnchorKind,
        value: &str,
    ) -> Result<EventId> {
        require_non_empty("actor_id", actor_id)?;
        require_non_empty("handle", handle)?;
        require_non_empty("value", value)?;

        if !self.project_exists(handle)? {
            return Err(
                CommandError::Invariant(format!("project does not exist: {handle}")).into(),
            );
        }

        if anchor_kind == ProjectAnchorKind::Rig {
            if let Some(owner) = self.rig_anchor_owner(value)? {
                if !same_identifier(&owner, handle) {
                    return Err(CommandError::Invariant(format!(
                        "rig anchor already claimed by project {owner}: {value}"
                    ))
                    .into());
                }
            }
        }

        let event = self.event_with_uuid(
            actor_id,
            EventPayload::ProjectAnchored(ProjectAnchorPayload {
                handle: handle.to_owned(),
                anchor_kind,
                value: value.to_owned(),
            }),
            None,
            Uuid::new_v4(),
        )?;

        self.append_event(event)
    }

    /// Remove a currently active anchor. Refused when no such anchor is active.
    pub fn unanchor_project(
        &self,
        actor_id: &str,
        handle: &str,
        anchor_kind: ProjectAnchorKind,
        value: &str,
    ) -> Result<EventId> {
        require_non_empty("actor_id", actor_id)?;
        require_non_empty("handle", handle)?;
        require_non_empty("value", value)?;

        if !self.project_anchor_exists(handle, anchor_kind, value)? {
            return Err(CommandError::Invariant(format!(
                "no active {} anchor on project {handle}: {value}",
                anchor_kind.as_str()
            ))
            .into());
        }

        let event = self.event_with_uuid(
            actor_id,
            EventPayload::ProjectUnanchored(ProjectAnchorPayload {
                handle: handle.to_owned(),
                anchor_kind,
                value: value.to_owned(),
            }),
            None,
            Uuid::new_v4(),
        )?;

        self.append_event(event)
    }

    pub fn record_ingest_batch(
        &self,
        actor_id: &str,
        batch_id: &str,
        agent_tool: &str,
        session_id: &str,
        turns: Vec<IngestTurn>,
    ) -> Result<EventId> {
        require_valid_actor_id(actor_id)?;
        require_non_empty("batch_id", batch_id)?;
        require_non_empty("agent_tool", agent_tool)?;
        require_non_empty("session_id", session_id)?;

        let event = self.event_with_uuid(
            actor_id,
            EventPayload::IngestBatchReceived(IngestBatchReceivedPayload {
                batch_id: batch_id.to_owned(),
                agent_tool: agent_tool.to_owned(),
                session_id: session_id.to_owned(),
                turns,
            }),
            None,
            Uuid::new_v4(),
        )?;

        self.append_event(event)
    }

    /// Records one classification covering one or more ingest batches
    /// (`batch_ids`, submission order; must be non-empty). A single event
    /// marks every listed batch classified together — the session-grouped
    /// path (hivemind-zdsh.18) uses this to cover a whole session's pending
    /// batches with one model call and one event, instead of one event per
    /// batch.
    pub fn record_ingest_batch_classified(
        &self,
        actor_id: &str,
        batch_ids: &[String],
        classifier_model: &str,
        schema_version: &str,
        captures: Vec<CaptureItem>,
        causation_event_id: Option<EventId>,
    ) -> Result<EventId> {
        require_valid_actor_id(actor_id)?;
        let Some(first_batch_id) = batch_ids.first() else {
            return Err(CommandError::Validation("batch_ids must not be empty".into()).into());
        };
        for batch_id in batch_ids {
            require_non_empty("batch_id", batch_id)?;
        }
        require_non_empty("classifier_model", classifier_model)?;
        require_non_empty("schema_version", schema_version)?;

        let event = self.event_with_uuid(
            actor_id,
            EventPayload::IngestBatchClassified(IngestBatchClassifiedPayload {
                batch_id: first_batch_id.clone(),
                batch_ids: batch_ids.to_vec(),
                classifier_model: classifier_model.to_owned(),
                schema_version: schema_version.to_owned(),
                captures,
            }),
            causation_event_id,
            Uuid::new_v4(),
        )?;

        self.append_event(event)
    }

    pub fn record_decision_scored(
        &self,
        actor_id: &str,
        payload: DecisionScoredPayload,
        causation_event_id: Option<EventId>,
    ) -> Result<EventId> {
        require_valid_actor_id(actor_id)?;
        require_non_empty("payload.capture_node_id", &payload.capture_node_id)?;
        require_non_empty("payload.scorer_model", &payload.scorer_model)?;
        require_non_empty("payload.weight_version", &payload.weight_version)?;

        let event = self.event_with_uuid(
            actor_id,
            EventPayload::DecisionScored(payload),
            causation_event_id,
            Uuid::new_v4(),
        )?;

        self.append_event(event)
    }

    pub fn record_option(
        &self,
        actor_id: &str,
        label: &str,
        description: &str,
    ) -> Result<OptionId> {
        require_valid_actor_id(actor_id)?;
        require_non_empty("label", label)?;
        require_non_empty("description", description)?;

        let option_id = generate_option_id();
        self.record_option_with_id(actor_id, &option_id, label, description)?;
        Ok(option_id)
    }

    pub fn record_option_with_id(
        &self,
        actor_id: &str,
        option_id: &str,
        label: &str,
        description: &str,
    ) -> Result<OptionId> {
        require_valid_actor_id(actor_id)?;
        require_non_empty("option_id", option_id)?;
        require_non_empty("label", label)?;
        require_non_empty("description", description)?;
        validate_option_label(label)?;

        let mut state = self.lock_state()?;
        state.option_ids.insert(option_id.to_owned());
        state
            .option_descriptions
            .insert(option_id.to_owned(), description.to_owned());
        Ok(option_id.to_owned())
    }

    pub fn propose_decision(&self, input: DecisionProposalInput<'_>) -> Result<DecisionId> {
        self.propose_decision_placed(input)
            .map(|(decision_id, _placement)| decision_id)
    }

    /// `propose_decision`, also returning where the decision was filed and how that was
    /// determined, so a capture reply can name its project without re-reading the ledger.
    pub fn propose_decision_placed(
        &self,
        input: DecisionProposalInput<'_>,
    ) -> Result<(DecisionId, DecisionPlacement)> {
        require_valid_actor_id(input.actor_id)?;
        validate_title("title", input.title)?;
        require_non_empty("rationale", input.rationale)?;

        if input.option_ids.is_empty() {
            return Err(CommandError::Validation("option_ids must not be empty".to_owned()).into());
        }

        if input.decided_by.is_some() && input.chosen_option_id.is_none() {
            return Err(CommandError::Validation(
                "decided_by requires a chosen_option_id — accepting a decision needs a decided option".to_owned(),
            )
            .into());
        }

        if input.still_proposed && input.decided_by.is_some() {
            return Err(CommandError::Validation(
                "still_proposed conflicts with decided_by — decided_by already asserts who decided"
                    .to_owned(),
            )
            .into());
        }

        require_quote_pairing(input.quote, input.question)?;
        require_readable_rationale(input.rationale, input.quote.is_some())?;
        require_valid_expressed_confidence(input.expressed_confidence)?;

        let normalized_topic_keys: Vec<String> = input
            .topic_keys
            .iter()
            .map(|topic| normalize_topic_key(topic))
            .filter(|topic| !topic.is_empty())
            .collect();

        if normalized_topic_keys.is_empty() {
            return Err(CommandError::Validation(
                "topic_keys must contain at least one non-empty normalized key".to_owned(),
            )
            .into());
        }

        {
            let state = self.lock_state()?;
            for option_id in input.option_ids {
                if !state.option_ids.contains(option_id) {
                    return Err(CommandError::Invariant(format!(
                        "option does not exist: {option_id}"
                    ))
                    .into());
                }
            }
        }

        if let Some(chosen_option_id) = input.chosen_option_id {
            let chosen_option_is_candidate = input.option_ids.iter().any(|option_id| {
                // ubs:ignore: option IDs are public decision graph IDs, not secrets.
                same_identifier(option_id, chosen_option_id)
            });
            if !chosen_option_is_candidate {
                return Err(CommandError::Validation(
                    "chosen_option_id must be one of option_ids".to_owned(),
                )
                .into());
            }
        }

        for hypothesis_id in input.hypothesis_ids {
            if !self.hypothesis_exists(hypothesis_id)? {
                return Err(CommandError::Invariant(format!(
                    "hypothesis does not exist: {hypothesis_id}"
                ))
                .into());
            }
        }

        for evidence_id in input.evidence_ids {
            if !self.evidence_exists(evidence_id)? {
                return Err(CommandError::Invariant(format!(
                    "evidence does not exist: {evidence_id}"
                ))
                .into());
            }
        }

        self.validate_grounding_premises(input.grounding)?;

        let event_uuids = DecisionProposalEventUuids {
            proposal: Uuid::new_v4(),
            has_option: repeat_uuid(input.option_ids.len()),
            chose: input.chosen_option_id.map(|_| Uuid::new_v4()),
            assumes: repeat_uuid(input.hypothesis_ids.len()),
            based_on: repeat_uuid(input.evidence_ids.len()),
            follows_from: repeat_uuid(input.grounding.premise_decision_ids().len()),
        };
        let decision_id = generate_entity_id("decision");

        let (project, project_source) = self.resolve_stated_project(input.project)?;
        let placement = DecisionPlacement::from_recorded(
            input.actor_id,
            project.as_deref(),
            Some(project_source),
        );
        self.propose_decision_with_id_and_project(
            input,
            &decision_id,
            event_uuids,
            project,
            project_source,
        )?;

        if !input.still_proposed && input.chosen_option_id.is_some() {
            let decider = input.decided_by.unwrap_or(input.actor_id);
            self.accept_decision(&decision_id, decider)?;
        }

        Ok((decision_id, placement))
    }

    pub fn propose_decision_with_id(
        &self,
        input: DecisionProposalInput<'_>,
        decision_id: &str,
        event_uuids: DecisionProposalEventUuids,
    ) -> Result<DecisionProposalEventIds> {
        let (project, project_source) = self.resolve_stated_project(input.project)?;
        self.propose_decision_with_id_and_project(
            input,
            decision_id,
            event_uuids,
            project,
            project_source,
        )
    }

    /// Core of `propose_decision_with_id`, taking an already-resolved `project`/
    /// `project_source` pair rather than re-deriving one from `input.project`. `supersede`
    /// calls this directly so a superseding decision can inherit its predecessor's
    /// `project_source` verbatim (e.g. `folder_marker`) instead of it being overwritten
    /// with `Stated` just because the inherited project happens to be a handle.
    fn propose_decision_with_id_and_project(
        &self,
        input: DecisionProposalInput<'_>,
        decision_id: &str,
        event_uuids: DecisionProposalEventUuids,
        project: Option<String>,
        project_source: ProjectSource,
    ) -> Result<DecisionProposalEventIds> {
        require_valid_actor_id(input.actor_id)?;
        require_non_empty("decision_id", decision_id)?;
        validate_title("title", input.title)?;
        require_non_empty("rationale", input.rationale)?;

        if input.option_ids.is_empty() {
            return Err(CommandError::Validation("option_ids must not be empty".to_owned()).into());
        }

        if input.option_labels.len() != input.option_ids.len() {
            return Err(CommandError::Validation(
                "option_labels must have exactly one label per option_id (hivemind-zdsh.10: a \
                 label is a required part of the option entity, not optional)"
                    .to_owned(),
            )
            .into());
        }

        if event_uuids.has_option.len() != input.option_ids.len() {
            return Err(CommandError::Validation(
                "has_option event UUID count must match option_ids".to_owned(),
            )
            .into());
        }

        if event_uuids.assumes.len() != input.hypothesis_ids.len() {
            return Err(CommandError::Validation(
                "assumes event UUID count must match hypothesis_ids".to_owned(),
            )
            .into());
        }

        if event_uuids.based_on.len() != input.evidence_ids.len() {
            return Err(CommandError::Validation(
                "based_on event UUID count must match evidence_ids".to_owned(),
            )
            .into());
        }

        if event_uuids.follows_from.len() != input.grounding.premise_decision_ids().len() {
            return Err(CommandError::Validation(
                "follows_from event UUID count must match grounding's premise_decision_ids"
                    .to_owned(),
            )
            .into());
        }

        if input.chosen_option_id.is_some() != event_uuids.chose.is_some() {
            return Err(CommandError::Validation(
                "chose event UUID must be present exactly when chosen_option_id is present"
                    .to_owned(),
            )
            .into());
        }

        require_quote_pairing(input.quote, input.question)?;
        require_readable_rationale(input.rationale, input.quote.is_some())?;
        require_valid_expressed_confidence(input.expressed_confidence)?;

        let normalized_topic_keys: Vec<String> = input
            .topic_keys
            .iter()
            .map(|topic| normalize_topic_key(topic))
            .filter(|topic| !topic.is_empty())
            .collect();

        if normalized_topic_keys.is_empty() {
            return Err(CommandError::Validation(
                "topic_keys must contain at least one non-empty normalized key".to_owned(),
            )
            .into());
        }

        let option_descriptions: Vec<String> = {
            let state = self.lock_state()?;
            let mut descriptions = Vec::with_capacity(input.option_ids.len());
            for option_id in input.option_ids {
                if !state.option_ids.contains(option_id) {
                    return Err(CommandError::Invariant(format!(
                        "option does not exist: {option_id}"
                    ))
                    .into());
                }
                // Recorded by `record_option_with_id` when this option_id was created; every
                // current caller records before proposing, so this is always populated for
                // ids that passed the existence check above. Empty string, not an error, for
                // the theoretical case of a pre-existing id that skipped that step.
                descriptions.push(
                    state
                        .option_descriptions
                        .get(option_id)
                        .cloned()
                        .unwrap_or_default(),
                );
            }
            descriptions
        };

        if let Some(chosen_option_id) = input.chosen_option_id {
            let chosen_option_is_candidate = input.option_ids.iter().any(|option_id| {
                // ubs:ignore: option IDs are public decision graph IDs, not secrets.
                same_identifier(option_id, chosen_option_id)
            });
            if !chosen_option_is_candidate {
                return Err(CommandError::Validation(
                    "chosen_option_id must be one of option_ids".to_owned(),
                )
                .into());
            }
        }

        for hypothesis_id in input.hypothesis_ids {
            if !self.hypothesis_exists(hypothesis_id)? {
                return Err(CommandError::Invariant(format!(
                    "hypothesis does not exist: {hypothesis_id}"
                ))
                .into());
            }
        }

        for evidence_id in input.evidence_ids {
            if !self.evidence_exists(evidence_id)? {
                return Err(CommandError::Invariant(format!(
                    "evidence does not exist: {evidence_id}"
                ))
                .into());
            }
        }

        if input.grounding.declared_and_empty() {
            return Err(CommandError::Validation(
                "grounding must name at least one premise decision, evidence item or hypothesis (a bet counts)".to_owned(),
            )
            .into());
        }

        let mut stale_premises = Vec::new();
        for premise_id in input.grounding.premise_decision_ids() {
            require_not_own_premise(decision_id, premise_id)?;
            self.require_decision_exists(premise_id)?;
            if self.decision_is_stale(premise_id)? {
                stale_premises.push(premise_id);
            }
        }
        let premise_stale: Vec<DecisionId> = stale_premises.into_iter().cloned().collect();

        let root_event = self.event_with_uuid(
            input.actor_id,
            EventPayload::DecisionProposed(DecisionProposedPayload {
                decision_id: decision_id.to_owned(),
                title: input.title.to_owned(),
                rationale: input.rationale.to_owned(),
                topic_keys: normalized_topic_keys,
                option_ids: input.option_ids.to_vec(),
                option_labels: input.option_labels.to_vec(),
                option_descriptions,
                chosen_option_id: input.chosen_option_id.map(ToOwned::to_owned),
                hypothesis_ids: input.hypothesis_ids.to_vec(),
                evidence_ids: input.evidence_ids.to_vec(),
                expressed_confidence: input.expressed_confidence.map(ToOwned::to_owned),
                quote: input.quote.map(ToOwned::to_owned),
                question: input.question.map(ToOwned::to_owned),
                project,
                project_source: Some(project_source),
            }),
            None,
            event_uuids.proposal,
        )?;

        let root_event_id = self.append_event(root_event)?;
        let mut relation_event_ids = Vec::new();

        for (option_id, event_uuid) in input.option_ids.iter().zip(event_uuids.has_option) {
            relation_event_ids.push(self.append_relation_event_with_uuid(
                input.actor_id,
                root_event_id,
                RelationKind::HasOption,
                decision_id,
                option_id,
                event_uuid,
            )?);
        }

        if let (Some(chosen_option_id), Some(event_uuid)) =
            (input.chosen_option_id, event_uuids.chose)
        {
            relation_event_ids.push(self.append_relation_event_with_uuid(
                input.actor_id,
                root_event_id,
                RelationKind::Chose,
                decision_id,
                chosen_option_id,
                event_uuid,
            )?);
        }

        let assumes_from_id = input.chosen_option_id.unwrap_or(decision_id);
        for (hypothesis_id, event_uuid) in input.hypothesis_ids.iter().zip(event_uuids.assumes) {
            relation_event_ids.push(self.append_relation_event_with_uuid(
                input.actor_id,
                root_event_id,
                RelationKind::Assumes,
                assumes_from_id,
                hypothesis_id,
                event_uuid,
            )?);
        }

        for (evidence_id, event_uuid) in input.evidence_ids.iter().zip(event_uuids.based_on) {
            relation_event_ids.push(self.append_relation_event_with_uuid(
                input.actor_id,
                root_event_id,
                RelationKind::BasedOn,
                decision_id,
                evidence_id,
                event_uuid,
            )?);
        }

        for (premise_id, event_uuid) in input
            .grounding
            .premise_decision_ids()
            .iter()
            .zip(event_uuids.follows_from)
        {
            relation_event_ids.push(self.append_relation_event_with_uuid(
                input.actor_id,
                root_event_id,
                RelationKind::FollowsFrom,
                decision_id,
                premise_id,
                event_uuid,
            )?);
        }

        Ok(DecisionProposalEventIds {
            proposal_event_id: root_event_id,
            relation_event_ids,
            premise_stale,
        })
    }

    pub fn accept_decision(&self, decision_id: &str, actor_id: &str) -> Result<EventId> {
        self.accept_decision_with_uuid(decision_id, actor_id, Uuid::new_v4())
    }

    pub fn accept_decision_with_uuid(
        &self,
        decision_id: &str,
        actor_id: &str,
        event_uuid: Uuid,
    ) -> Result<EventId> {
        require_valid_actor_id(actor_id)?;
        require_non_empty("decision_id", decision_id)?;

        if !self.decision_exists(decision_id)? {
            return Err(
                CommandError::Invariant(format!("decision does not exist: {decision_id}")).into(),
            );
        }

        if self.actor_has_decision_event(decision_id, actor_id, EventType::DecisionRejected)? {
            return Err(CommandError::Invariant(format!(
                "actor {actor_id} cannot accept and reject decision {decision_id}"
            ))
            .into());
        }

        let event = self.event_with_uuid(
            actor_id,
            EventPayload::DecisionAccepted(DecisionIdPayload {
                decision_id: decision_id.to_owned(),
            }),
            None,
            event_uuid,
        )?;

        self.append_event(event)
    }

    pub fn reject_decision(&self, decision_id: &str, actor_id: &str) -> Result<EventId> {
        self.reject_decision_with_uuid(decision_id, actor_id, Uuid::new_v4())
    }

    pub fn disagree(&self, actor_id: &str, decision_id: &str, reason: &str) -> Result<EventId> {
        self.disagree_with_uuid(actor_id, decision_id, reason, Uuid::new_v4())
    }

    pub fn disagree_with_uuid(
        &self,
        actor_id: &str,
        decision_id: &str,
        reason: &str,
        event_uuid: Uuid,
    ) -> Result<EventId> {
        require_valid_actor_id(actor_id)?;
        require_non_empty("decision_id", decision_id)?;
        require_non_empty("reason", reason)?;

        if !self.decision_exists(decision_id)? {
            return Err(
                CommandError::Invariant(format!("decision does not exist: {decision_id}")).into(),
            );
        }

        if let Some(existing_event_id) =
            self.find_decision_event_id(decision_id, actor_id, EventType::DecisionRejected)?
        {
            return Ok(existing_event_id);
        }

        if self.actor_has_decision_event(decision_id, actor_id, EventType::DecisionAccepted)? {
            return Err(CommandError::Invariant(format!(
                "actor {actor_id} cannot accept and reject decision {decision_id}"
            ))
            .into());
        }

        let event = self.event_with_uuid(
            actor_id,
            EventPayload::DecisionRejected(DecisionRejectedPayload {
                decision_id: decision_id.to_owned(),
                reason: Some(reason.to_owned()),
            }),
            None,
            event_uuid,
        )?;

        self.append_event(event)
    }

    pub fn reject_decision_with_uuid(
        &self,
        decision_id: &str,
        actor_id: &str,
        event_uuid: Uuid,
    ) -> Result<EventId> {
        require_valid_actor_id(actor_id)?;
        require_non_empty("decision_id", decision_id)?;

        if !self.decision_exists(decision_id)? {
            return Err(
                CommandError::Invariant(format!("decision does not exist: {decision_id}")).into(),
            );
        }

        if self.actor_has_decision_event(decision_id, actor_id, EventType::DecisionAccepted)? {
            return Err(CommandError::Invariant(format!(
                "actor {actor_id} cannot accept and reject decision {decision_id}"
            ))
            .into());
        }

        let event = self.event_with_uuid(
            actor_id,
            EventPayload::DecisionRejected(DecisionRejectedPayload {
                decision_id: decision_id.to_owned(),
                reason: None,
            }),
            None,
            event_uuid,
        )?;

        self.append_event(event)
    }

    pub fn supersede_decision(
        &self,
        old_decision_id: &str,
        new_decision_id: &str,
        actor_id: &str,
    ) -> Result<EventId> {
        self.supersede_decision_with_uuid(
            old_decision_id,
            new_decision_id,
            actor_id,
            Uuid::new_v4(),
        )
    }

    pub fn supersede_decision_with_uuid(
        &self,
        old_decision_id: &str,
        new_decision_id: &str,
        actor_id: &str,
        event_uuid: Uuid,
    ) -> Result<EventId> {
        require_valid_actor_id(actor_id)?;
        require_non_empty("old_decision_id", old_decision_id)?;
        require_non_empty("new_decision_id", new_decision_id)?;

        if same_identifier(old_decision_id, new_decision_id) {
            return Err(CommandError::Validation(
                "old_decision_id and new_decision_id must be different".to_owned(),
            )
            .into());
        }

        if !self.decision_exists(old_decision_id)? {
            return Err(CommandError::Invariant(format!(
                "decision does not exist: {old_decision_id}"
            ))
            .into());
        }

        if !self.decision_exists(new_decision_id)? {
            return Err(CommandError::Invariant(format!(
                "decision does not exist: {new_decision_id}"
            ))
            .into());
        }

        let event = self.event_with_uuid(
            actor_id,
            EventPayload::DecisionSuperseded(DecisionSupersededPayload {
                old_decision_id: old_decision_id.to_owned(),
                new_decision_id: new_decision_id.to_owned(),
            }),
            None,
            event_uuid,
        )?;

        self.append_event(event)
    }

    pub fn supersede(&self, input: SupersedeInput<'_>) -> Result<SupersedeOutcome> {
        require_valid_actor_id(input.actor_id)?;
        require_non_empty("old_decision_id", input.old_decision_id)?;
        validate_title("new_title", input.new_title)?;
        require_non_empty("new_rationale", input.new_rationale)?;
        require_optional_non_empty("chosen_option_label", input.chosen_option_label)?;

        let old_decision = self
            .decision_proposal_snapshot(input.old_decision_id)?
            .ok_or_else(|| {
                CommandError::Invariant(format!(
                    "decision does not exist: {}",
                    input.old_decision_id
                ))
            })?;
        let effective_topic_keys =
            effective_topic_keys(input.topic_keys, old_decision.topic_keys.as_slice())?;
        let option_labels = effective_option_labels(
            input.new_title,
            input.option_labels,
            input.chosen_option_label,
        )?;
        let option_ids = deterministic_supersede_option_ids(
            input.actor_id,
            input.old_decision_id,
            input.new_title,
            input.new_rationale,
            &option_labels,
        );
        let chosen_label = input.chosen_option_label.map(str::trim);
        let chosen_option_id = chosen_label
            .map(|label| {
                option_labels
                    .iter()
                    .position(|option_label| option_label == label)
                    .map(|index| option_ids[index].clone())
                    .ok_or_else(|| {
                        CommandError::Validation(
                            "chosen_option_label must be one of option_labels".to_owned(),
                        )
                    })
            })
            .transpose()?;

        // The new decision inherits the old decision's project verbatim (both the handle
        // and how it was determined) unless this call states one explicitly -- an
        // inherited `folder_marker`/`rig`/etc. project_source stays truthful across the
        // chain rather than being overwritten with `stated` just because a handle is
        // present.
        let (project, project_source) = match input.project {
            Some(determined) => self.resolve_stated_project(Some(determined))?,
            None => (
                old_decision.project.clone(),
                old_decision
                    .project_source
                    .unwrap_or(ProjectSource::PersonalFallback),
            ),
        };
        let placement = DecisionPlacement::from_recorded(
            input.actor_id,
            project.as_deref(),
            Some(project_source),
        );

        let proposal_props = DecisionProposalInput {
            grounding: Grounding::NotAsked,
            expressed_confidence: None,
            actor_id: input.actor_id,
            title: input.new_title,
            rationale: input.new_rationale,
            topic_keys: &effective_topic_keys,
            option_ids: &option_ids,
            option_labels: &option_labels,
            chosen_option_id: chosen_option_id.as_deref(),
            decided_by: None,
            // Unread here: this goes through `propose_decision_with_id`, which never
            // auto-accepts (only `propose_decision` does). Superseding decisions stay
            // `proposed` until separately accepted — out of scope for hivemind-zdsh.8.
            still_proposed: true,
            hypothesis_ids: input.hypothesis_ids,
            evidence_ids: input.evidence_ids,
            // Superseding decisions don't carry a quote/question in this slice
            // (hivemind-zdsh.13 scoped this to decision.capture/decision.proposed).
            quote: None,
            question: None,
            project: project.as_deref().map(|handle| DeterminedProject {
                handle,
                source: project_source,
            }),
        };
        if let Some(existing) =
            self.find_matching_supersede(input.old_decision_id, &proposal_props)?
        {
            return Ok(existing);
        }

        for (option_label, option_id) in option_labels.iter().zip(&option_ids) {
            let mut option_description = String::with_capacity(
                "Option generated from supersede value ''".len() + option_label.len(),
            );
            let _ = write!(
                option_description,
                "Option generated from supersede value '{option_label}'"
            );
            self.record_option_with_id(
                input.actor_id,
                option_id,
                option_label,
                &option_description,
            )?;
        }

        let new_decision_id = generate_entity_id("decision");
        let proposal_event_ids = self.propose_decision_with_id_and_project(
            proposal_props,
            &new_decision_id,
            DecisionProposalEventUuids {
                proposal: Uuid::new_v4(),
                has_option: repeat_uuid(option_ids.len()),
                chose: chosen_option_id.as_ref().map(|_| Uuid::new_v4()),
                assumes: repeat_uuid(input.hypothesis_ids.len()),
                based_on: repeat_uuid(input.evidence_ids.len()),
                // Superseding decisions don't name premises in this slice (see the
                // `grounding: Grounding::NotAsked` comment on `proposal_props` above).
                follows_from: Vec::new(),
            },
            project.clone(),
            project_source,
        )?;
        let superseded_event_id = self.supersede_decision_with_uuid(
            input.old_decision_id,
            &new_decision_id,
            input.actor_id,
            Uuid::new_v4(),
        )?;

        Ok(SupersedeOutcome {
            new_decision_id,
            proposal_event_id: proposal_event_ids.proposal_event_id,
            relation_event_ids: proposal_event_ids.relation_event_ids,
            superseded_event_id,
            placement,
        })
    }

    pub fn attach_evidence(
        &self,
        decision_id: &str,
        evidence_id: &str,
        actor_id: &str,
    ) -> Result<EventId> {
        self.attach_evidence_with_uuid(decision_id, evidence_id, actor_id, Uuid::new_v4())
    }

    pub fn attach_evidence_with_uuid(
        &self,
        decision_id: &str,
        evidence_id: &str,
        actor_id: &str,
        event_uuid: Uuid,
    ) -> Result<EventId> {
        require_valid_actor_id(actor_id)?;
        require_non_empty("decision_id", decision_id)?;
        require_non_empty("evidence_id", evidence_id)?;

        if !self.decision_exists(decision_id)? {
            return Err(
                CommandError::Invariant(format!("decision does not exist: {decision_id}")).into(),
            );
        }
        if !self.evidence_exists(evidence_id)? {
            let hint = if self.decision_exists(evidence_id)? {
                " (that id is a decision — use FOLLOWS_FROM to link a decision to a decision it follows from, not BASED_ON)"
            } else {
                ""
            };
            return Err(CommandError::Invariant(format!(
                "evidence does not exist: {evidence_id}{hint}"
            ))
            .into());
        }

        self.append_relation_event_with_uuid(
            actor_id,
            0,
            RelationKind::BasedOn,
            decision_id,
            evidence_id,
            event_uuid,
        )
    }

    /// Links `decision_id` to `premise_decision_id` as a `FOLLOWS_FROM` premise, without
    /// causation (this is always a standalone link, never part of a proposal's fan-out —
    /// see `propose_decision_with_id` for the at-capture case). Existence and not-self only;
    /// cycle refusal is a verb-level rule that needs a graph walk (gwhr.4's `ground` CLI).
    pub fn link_follows_from(
        &self,
        decision_id: &str,
        premise_decision_id: &str,
        actor_id: &str,
    ) -> Result<EventId> {
        self.link_follows_from_with_uuid(decision_id, premise_decision_id, actor_id, Uuid::new_v4())
    }

    pub fn link_follows_from_with_uuid(
        &self,
        decision_id: &str,
        premise_decision_id: &str,
        actor_id: &str,
        event_uuid: Uuid,
    ) -> Result<EventId> {
        require_valid_actor_id(actor_id)?;
        require_non_empty("decision_id", decision_id)?;
        require_non_empty("premise_decision_id", premise_decision_id)?;

        require_not_own_premise(decision_id, premise_decision_id)?;
        self.require_decision_exists(decision_id)?;
        self.require_decision_exists(premise_decision_id)?;

        self.append_relation_event_with_uuid(
            actor_id,
            0,
            RelationKind::FollowsFrom,
            decision_id,
            premise_decision_id,
            event_uuid,
        )
    }

    /// Give an existing decision its grounding after the fact, attributed to `actor_id`
    /// (never the proposal's actor unless they're the same). Appends `FOLLOWS_FROM` /
    /// `BASED_ON` / `ASSUMES` with no causation — see `Grounding` for why "at capture" vs
    /// "later" needs no new field. Existence and not-self checks only; cycle refusal for
    /// premises is a verb-level rule the `ground` CLI (gwhr.4) applies before calling this,
    /// since it needs a graph walk this ledger-only layer doesn't have.
    pub fn ground_decision(&self, input: GroundInput<'_>) -> Result<Vec<EventId>> {
        require_valid_actor_id(input.actor_id)?;
        require_non_empty("decision_id", input.decision_id)?;

        if input.premise_decision_ids.is_empty()
            && input.evidence_ids.is_empty()
            && input.hypothesis_ids.is_empty()
        {
            return Err(CommandError::Validation(
                "grounding must name at least one premise decision, evidence item or hypothesis (a bet counts)".to_owned(),
            )
            .into());
        }

        self.require_decision_exists(input.decision_id)?;

        for premise_id in input.premise_decision_ids {
            require_not_own_premise(input.decision_id, premise_id)?;
            self.require_decision_exists(premise_id)?;
        }
        for evidence_id in input.evidence_ids {
            self.require_evidence_exists(evidence_id)?;
        }
        for hypothesis_id in input.hypothesis_ids {
            self.require_hypothesis_exists(hypothesis_id)?;
        }

        let mut event_ids = Vec::new();
        for premise_id in input.premise_decision_ids {
            event_ids.push(self.append_relation_event(
                input.actor_id,
                0,
                RelationKind::FollowsFrom,
                input.decision_id,
                premise_id,
            )?);
        }
        for evidence_id in input.evidence_ids {
            event_ids.push(self.append_relation_event(
                input.actor_id,
                0,
                RelationKind::BasedOn,
                input.decision_id,
                evidence_id,
            )?);
        }
        let assumes_from_id = self
            .chosen_option_for_decision(input.decision_id)?
            .unwrap_or_else(|| input.decision_id.to_owned());
        for hypothesis_id in input.hypothesis_ids {
            event_ids.push(self.append_relation_event(
                input.actor_id,
                0,
                RelationKind::Assumes,
                &assumes_from_id,
                hypothesis_id,
            )?);
        }

        Ok(event_ids)
    }

    pub fn assume_hypothesis_with_uuid(
        &self,
        decision_id: &str,
        hypothesis_id: &str,
        actor_id: &str,
        event_uuid: Uuid,
    ) -> Result<EventId> {
        require_valid_actor_id(actor_id)?;
        require_non_empty("decision_id", decision_id)?;
        require_non_empty("hypothesis_id", hypothesis_id)?;

        if !self.decision_exists(decision_id)? {
            return Err(
                CommandError::Invariant(format!("decision does not exist: {decision_id}")).into(),
            );
        }
        if !self.hypothesis_exists(hypothesis_id)? {
            return Err(CommandError::Invariant(format!(
                "hypothesis does not exist: {hypothesis_id}"
            ))
            .into());
        }

        let from_id = self
            .chosen_option_for_decision(decision_id)?
            .unwrap_or_else(|| decision_id.to_owned());
        self.append_relation_event_with_uuid(
            actor_id,
            0,
            RelationKind::Assumes,
            &from_id,
            hypothesis_id,
            event_uuid,
        )
    }

    pub fn relate_evidence_to_hypothesis(
        &self,
        evidence_id: &str,
        hypothesis_id: &str,
        relation_kind: RelationKind,
        actor_id: &str,
    ) -> Result<EventId> {
        require_valid_actor_id(actor_id)?;
        require_non_empty("evidence_id", evidence_id)?;
        require_non_empty("hypothesis_id", hypothesis_id)?;

        if !matches!(
            relation_kind,
            RelationKind::Supports | RelationKind::Refutes
        ) {
            return Err(CommandError::Validation(
                "relation kind must be supports or refutes".to_owned(),
            )
            .into());
        }

        if !self.evidence_exists(evidence_id)? {
            return Err(
                CommandError::Invariant(format!("evidence does not exist: {evidence_id}")).into(),
            );
        }
        if !self.hypothesis_exists(hypothesis_id)? {
            return Err(CommandError::Invariant(format!(
                "hypothesis does not exist: {hypothesis_id}"
            ))
            .into());
        }

        if let Some(existing_event_id) =
            self.find_relation_event_id(relation_kind, evidence_id, hypothesis_id)?
        {
            return Ok(existing_event_id);
        }

        self.append_relation_event(actor_id, 0, relation_kind, evidence_id, hypothesis_id)
    }

    fn append_relation_event(
        &self,
        actor_id: &str,
        root_event_id: EventId,
        relation: RelationKind,
        from_id: &str,
        to_id: &str,
    ) -> Result<EventId> {
        self.append_relation_event_with_uuid(
            actor_id,
            root_event_id,
            relation,
            from_id,
            to_id,
            Uuid::new_v4(),
        )
    }

    fn append_relation_event_with_uuid(
        &self,
        actor_id: &str,
        root_event_id: EventId,
        relation: RelationKind,
        from_id: &str,
        to_id: &str,
        event_uuid: Uuid,
    ) -> Result<EventId> {
        let event = self.event_with_uuid(
            actor_id,
            EventPayload::RelationAdded(RelationAddedPayload {
                relation,
                from_id: from_id.to_owned(),
                to_id: to_id.to_owned(),
            }),
            if root_event_id == 0 {
                None
            } else {
                Some(root_event_id)
            },
            event_uuid,
        )?;

        self.append_event(event)
    }

    fn event_with_uuid(
        &self,
        actor_id: &str,
        payload: EventPayload,
        causation_event_id: Option<EventId>,
        event_uuid: Uuid,
    ) -> Result<Event> {
        EventBuilder::new(event_uuid, actor_id, payload)
            .tenant_id(self.context.tenant_id.clone())
            .provenance(self.context.provenance.clone())
            .causation_event_id(causation_event_id)
            .timestamp(Some(Utc::now()))
            .build()
            .map_err(|error| {
                CommandError::Invariant(format!("failed to build typed event envelope: {error}"))
                    .into()
            })
    }

    fn lock_state(&self) -> Result<MutexGuard<'_, CommandState>> {
        self.state.lock().map_err(|error| {
            CommandError::Invariant(format!("commands state lock poisoned: {error}")).into()
        })
    }

    fn append_event(&self, event: Event) -> Result<EventId> {
        self.ledger
            .append_for_tenant(&self.context.tenant_id, event)
    }

    fn evidence_exists(&self, evidence_id: &str) -> Result<bool> {
        self.scan_events(|event| {
            // ubs:ignore: evidence IDs are public ledger IDs, not timing-sensitive secrets.
            let has_matching_id = payload_value_matches(event, "evidence_id", evidence_id);
            event.event_type == EventType::EvidenceRecorded && has_matching_id
        })
    }

    fn hypothesis_exists(&self, hypothesis_id: &str) -> Result<bool> {
        self.scan_events(|event| {
            let has_matching_id =
                // ubs:ignore: hypothesis IDs are public ledger IDs, not timing-sensitive secrets.
                payload_value_matches(event, "hypothesis_id", hypothesis_id);
            event.event_type == EventType::HypothesisRecorded && has_matching_id
        })
    }

    /// Resolve a decision's `project`/`project_source` pair from a caller-determined
    /// project (the write rule: a given handle must be registered, else a refusal naming
    /// the handle and the register command; no handle means personal fallback). The caller
    /// says how it got the handle, but only for the ways a caller can: `personal_fallback`
    /// is recorded by this layer when no handle is given, and `moved` only by a move, so
    /// neither may accompany a handle. Callers that need to inherit an existing decision's
    /// project instead of restating one (see `supersede`) bypass this and carry the
    /// inherited pair through directly.
    fn resolve_stated_project(
        &self,
        project: Option<DeterminedProject<'_>>,
    ) -> Result<(Option<String>, ProjectSource)> {
        let Some(DeterminedProject { handle, source }) = project else {
            return Ok((None, ProjectSource::PersonalFallback));
        };

        if matches!(
            source,
            ProjectSource::PersonalFallback | ProjectSource::Moved
        ) {
            return Err(CommandError::Validation(format!(
                "project_source \"{}\" cannot accompany a project handle: personal_fallback is recorded when no project is given, and moved only by moving a decision",
                source.as_str()
            ))
            .into());
        }

        if handle.starts_with(PERSONAL_PROJECT_HANDLE_PREFIX) {
            return Err(CommandError::Validation(format!(
                "project must not use the reserved \"{PERSONAL_PROJECT_HANDLE_PREFIX}\" prefix -- personal projects are derived from the actor, never stated: {handle}"
            ))
            .into());
        }

        if !self.project_exists(handle)? {
            return Err(CommandError::Invariant(format!(
                "project not registered: {handle} -- register it first with `hivemind project register {handle}`"
            ))
            .into());
        }

        Ok((Some(handle.to_owned()), source))
    }

    fn project_exists(&self, handle: &str) -> Result<bool> {
        self.scan_events(|event| {
            // ubs:ignore: project handles are public ledger IDs, not timing-sensitive secrets.
            let has_matching_handle = payload_value_matches(event, "handle", handle);
            event.event_type == EventType::ProjectRegistered && has_matching_handle
        })
    }

    /// Registration is permanent in this slice (no `project.unregistered` event exists),
    /// so the first match is the only match — same one-pass-to-first-hit shape as
    /// `find_decision_event_id` above.
    fn find_project_registered(&self, handle: &str) -> Result<Option<ProjectRegisteredPayload>> {
        let mut offset = 0;
        const PAGE_SIZE: usize = 1024;

        loop {
            let events = self
                .ledger
                .read_for_tenant(&self.context.tenant_id, offset, PAGE_SIZE)?;
            if events.is_empty() {
                return Ok(None);
            }

            for event in &events {
                if event.event_type != EventType::ProjectRegistered {
                    continue;
                }
                let Some(event_handle) = payload_value_as_str(event, "handle") else {
                    continue;
                };
                if same_identifier(event_handle, handle) {
                    let payload: ProjectRegisteredPayload =
                        // ubs:ignore: clone necessary — event borrowed from paginated Vec, from_value needs owned Value
                        serde_json::from_value(event.payload.clone()).map_err(|error| {
                            CommandError::Invariant(format!(
                                "corrupt project.registered payload for handle {handle}: {error}"
                            ))
                        })?;
                    return Ok(Some(payload));
                }
            }

            if let Some(last_event_id) = events.last().and_then(|event| event.event_id) {
                offset = last_event_id;
            } else {
                return Ok(None);
            }
        }
    }

    /// Currently active `project.linked` facts: every link added, minus every link that a
    /// later `project.unlinked` retracted. Single streaming pass, same shape as
    /// `find_matching_supersede`'s scan below.
    fn active_project_links(&self) -> Result<Vec<(String, String, ProjectLinkKind)>> {
        let mut links: Vec<(String, String, ProjectLinkKind)> = Vec::new();

        self.ledger
            .replay_from_for_tenant(&self.context.tenant_id, 0, &mut |event| {
                match event.event_type {
                    EventType::ProjectLinked => {
                        if let Ok(payload) =
                            serde_json::from_value::<ProjectLinkPayload>(event.payload.clone())
                        {
                            links.push((payload.from, payload.to, payload.kind));
                        }
                    }
                    EventType::ProjectUnlinked => {
                        if let Ok(payload) =
                            serde_json::from_value::<ProjectLinkPayload>(event.payload.clone())
                        {
                            links.retain(|(from, to, kind)| {
                                !(same_identifier(from, &payload.from)
                                    && same_identifier(to, &payload.to)
                                    && *kind == payload.kind)
                            });
                        }
                    }
                    _ => {}
                }
                Ok(())
            })?;

        Ok(links)
    }

    fn project_part_of_parent(&self, handle: &str) -> Result<Option<String>> {
        Ok(self
            .active_project_links()?
            .into_iter()
            .find(|(from, _, kind)| {
                *kind == ProjectLinkKind::PartOf && same_identifier(from, handle)
            })
            .map(|(_, to, _)| to))
    }

    fn project_link_exists(&self, from: &str, to: &str, kind: ProjectLinkKind) -> Result<bool> {
        Ok(self
            .active_project_links()?
            .into_iter()
            .any(|(link_from, link_to, link_kind)| {
                same_identifier(&link_from, from)
                    && same_identifier(&link_to, to)
                    && link_kind == kind
            }))
    }

    /// Currently active `project.anchored` facts: every anchor added, minus every anchor
    /// that a later `project.unanchored` retracted. Same net-of-adds-and-removes shape as
    /// `active_project_links`.
    fn active_project_anchors(&self) -> Result<Vec<(String, ProjectAnchorKind, String)>> {
        let mut anchors: Vec<(String, ProjectAnchorKind, String)> = Vec::new();

        self.ledger
            .replay_from_for_tenant(&self.context.tenant_id, 0, &mut |event| {
                match event.event_type {
                    EventType::ProjectAnchored => {
                        if let Ok(payload) =
                            serde_json::from_value::<ProjectAnchorPayload>(event.payload.clone())
                        {
                            anchors.push((payload.handle, payload.anchor_kind, payload.value));
                        }
                    }
                    EventType::ProjectUnanchored => {
                        if let Ok(payload) =
                            serde_json::from_value::<ProjectAnchorPayload>(event.payload.clone())
                        {
                            anchors.retain(|(handle, anchor_kind, value)| {
                                !(same_identifier(handle, &payload.handle)
                                    && *anchor_kind == payload.anchor_kind
                                    && same_identifier(value, &payload.value))
                            });
                        }
                    }
                    _ => {}
                }
                Ok(())
            })?;

        Ok(anchors)
    }

    fn rig_anchor_owner(&self, value: &str) -> Result<Option<String>> {
        Ok(self
            .active_project_anchors()?
            .into_iter()
            .find(|(_, anchor_kind, anchor_value)| {
                *anchor_kind == ProjectAnchorKind::Rig && same_identifier(anchor_value, value)
            })
            .map(|(handle, _, _)| handle))
    }

    fn project_anchor_exists(
        &self,
        handle: &str,
        anchor_kind: ProjectAnchorKind,
        value: &str,
    ) -> Result<bool> {
        Ok(self
            .active_project_anchors()?
            .into_iter()
            .any(|(anchor_handle, kind, anchor_value)| {
                same_identifier(&anchor_handle, handle)
                    && kind == anchor_kind
                    && same_identifier(&anchor_value, value)
            }))
    }

    fn decision_exists(&self, decision_id: &str) -> Result<bool> {
        self.scan_events(|event| {
            // ubs:ignore: decision IDs are public ledger IDs, not timing-sensitive secrets.
            let has_matching_id = payload_value_matches(event, "decision_id", decision_id);
            event.event_type == EventType::DecisionProposed && has_matching_id
        })
    }

    /// A premise decision is stale once it has been superseded (as the old side of a
    /// `decision.superseded`) or rejected. Append-only: staleness is reported, never a
    /// reason to refuse the link — honesty about it is the renderer's job (`premise_stale`
    /// on `DecisionProposalEventIds`, `PremiseSuperseded`/`PremiseRejected` in gwhr.3).
    fn decision_is_stale(&self, decision_id: &str) -> Result<bool> {
        self.scan_events(|event| match event.event_type {
            // ubs:ignore: decision IDs are public ledger IDs, not timing-sensitive secrets.
            EventType::DecisionSuperseded => {
                payload_value_matches(event, "old_decision_id", decision_id)
            }
            EventType::DecisionRejected => payload_value_matches(event, "decision_id", decision_id),
            _ => false,
        })
    }

    /// Shared "declared but empty" and premise-existence validation, run by both
    /// `propose_decision` (fail fast before generating the decision id) and
    /// `propose_decision_with_id` (which additionally refuses a self-premise and collects
    /// `premise_stale` once the real decision id exists).
    fn validate_grounding_premises(&self, grounding: Grounding<'_>) -> Result<()> {
        if grounding.declared_and_empty() {
            return Err(CommandError::Validation(
                "grounding must name at least one premise decision, evidence item or hypothesis (a bet counts)".to_owned(),
            )
            .into());
        }
        for premise_id in grounding.premise_decision_ids() {
            self.require_decision_exists(premise_id)?;
        }
        Ok(())
    }

    /// `Ok` when the tenant's ledger holds a `decision.proposed` for `decision_id`, else the
    /// `decision does not exist` invariant error. One place for the message so callers in a
    /// loop over premise ids stay allocation-free on the happy path.
    fn require_decision_exists(&self, decision_id: &str) -> Result<()> {
        if self.decision_exists(decision_id)? {
            return Ok(());
        }
        Err(CommandError::Invariant(format!("decision does not exist: {decision_id}")).into())
    }

    fn require_evidence_exists(&self, evidence_id: &str) -> Result<()> {
        if self.evidence_exists(evidence_id)? {
            return Ok(());
        }
        Err(CommandError::Invariant(format!("evidence does not exist: {evidence_id}")).into())
    }

    fn require_hypothesis_exists(&self, hypothesis_id: &str) -> Result<()> {
        if self.hypothesis_exists(hypothesis_id)? {
            return Ok(());
        }
        Err(CommandError::Invariant(format!("hypothesis does not exist: {hypothesis_id}")).into())
    }

    fn actor_has_decision_event(
        &self,
        decision_id: &str,
        actor_id: &str,
        decision_event_type: EventType,
    ) -> Result<bool> {
        Ok(self
            .find_decision_event_id(decision_id, actor_id, decision_event_type)?
            .is_some())
    }

    fn find_decision_event_id(
        &self,
        decision_id: &str,
        actor_id: &str,
        decision_event_type: EventType,
    ) -> Result<Option<EventId>> {
        let mut offset = 0;
        const PAGE_SIZE: usize = 1024;

        loop {
            let events = self
                .ledger
                .read_for_tenant(&self.context.tenant_id, offset, PAGE_SIZE)?;
            if events.is_empty() {
                return Ok(None);
            }

            for event in &events {
                // ubs:ignore: actor IDs are public ledger IDs, not timing-sensitive secrets.
                let has_matching_actor = same_identifier(event.actor_id.as_str(), actor_id);
                // ubs:ignore: decision IDs are public ledger IDs, not timing-sensitive secrets.
                let has_matching_id = payload_value_matches(event, "decision_id", decision_id);
                if event.event_type == decision_event_type && has_matching_actor && has_matching_id
                {
                    return Ok(event.event_id);
                }
            }

            if let Some(last_event_id) = events.last().and_then(|event| event.event_id) {
                offset = last_event_id;
            } else {
                return Ok(None);
            }
        }
    }

    fn decision_proposal_snapshot(
        &self,
        decision_id: &str,
    ) -> Result<Option<DecisionProposalSnapshot>> {
        let mut offset = 0;
        const PAGE_SIZE: usize = 1024;

        loop {
            let events = self
                .ledger
                .read_for_tenant(&self.context.tenant_id, offset, PAGE_SIZE)?;
            if events.is_empty() {
                return Ok(None);
            }

            for event in &events {
                if event.event_type != EventType::DecisionProposed {
                    continue;
                }
                let Some(snapshot) = decision_proposal_snapshot_from_event(event) else {
                    continue;
                };
                if same_identifier(snapshot.decision_id.as_str(), decision_id) {
                    return Ok(Some(snapshot));
                }
            }

            if let Some(last_event_id) = events.last().and_then(|event| event.event_id) {
                offset = last_event_id;
            } else {
                return Ok(None);
            }
        }
    }

    pub fn chosen_option_for_decision(&self, decision_id: &str) -> Result<Option<String>> {
        Ok(self
            .decision_proposal_snapshot(decision_id)?
            .and_then(|s| s.chosen_option_id))
    }

    fn find_matching_supersede(
        &self,
        old_decision_id: &str,
        props: &DecisionProposalInput<'_>,
    ) -> Result<Option<SupersedeOutcome>> {
        let mut proposals = HashMap::new();
        let mut superseded_events = Vec::new();
        let mut relation_event_ids_by_causation: HashMap<EventId, Vec<EventId>> = HashMap::new();

        // Single streaming pass — this function must collect all DecisionProposed,
        // DecisionSuperseded, and RelationAdded events before it can reason about
        // supersede idempotency, so there is no early-exit opportunity.
        self.ledger
            .replay_from_for_tenant(&self.context.tenant_id, 0, &mut |event| {
                match event.event_type {
                    EventType::DecisionProposed => {
                        if let Some(snapshot) = decision_proposal_snapshot_from_event(event) {
                            proposals.insert(snapshot.decision_id.clone(), snapshot);
                        }
                    }
                    EventType::DecisionSuperseded => {
                        let Some(event_id) = event.event_id else {
                            return Ok(());
                        };
                        let Some(old_id) = payload_value_as_str(event, "old_decision_id") else {
                            return Ok(());
                        };
                        let Some(new_id) = payload_value_as_str(event, "new_decision_id") else {
                            return Ok(());
                        };
                        // ubs:ignore: actor and decision IDs are public graph IDs.
                        if same_identifier(event.actor_id.as_str(), props.actor_id)
                            && same_identifier(old_id, old_decision_id)
                        {
                            superseded_events.push((event_id, new_id.to_owned()));
                        }
                    }
                    EventType::RelationAdded => {
                        if let (Some(causation_event_id), Some(event_id)) =
                            (event.causation_event_id, event.event_id)
                        {
                            relation_event_ids_by_causation
                                .entry(causation_event_id)
                                .or_default()
                                .push(event_id);
                        }
                    }
                    _ => {}
                }
                Ok(())
            })?;

        for (superseded_event_id, new_decision_id) in superseded_events {
            let Some(proposal) = proposals.get(&new_decision_id) else {
                continue;
            };
            // ubs:ignore: actor and decision IDs are public graph IDs.
            if !same_identifier(proposal.actor_id.as_str(), props.actor_id) {
                continue;
            }
            if proposal.title == props.title
                && proposal.rationale == props.rationale
                && proposal.topic_keys == props.topic_keys
                && proposal.option_ids == props.option_ids
                && proposal.chosen_option_id.as_deref() == props.chosen_option_id
                && proposal.hypothesis_ids == props.hypothesis_ids
                && proposal.evidence_ids == props.evidence_ids
                && proposal.project.as_deref() == props.project.map(|project| project.handle)
            {
                let relation_event_ids = relation_event_ids_by_causation
                    .get(&proposal.event_id)
                    .cloned()
                    .unwrap_or_default();
                let placement = DecisionPlacement::from_recorded(
                    &proposal.actor_id,
                    proposal.project.as_deref(),
                    proposal.project_source,
                );
                return Ok(Some(SupersedeOutcome {
                    new_decision_id,
                    proposal_event_id: proposal.event_id,
                    relation_event_ids,
                    superseded_event_id,
                    placement,
                }));
            }
        }

        Ok(None)
    }

    fn find_relation_event_id(
        &self,
        relation_kind: RelationKind,
        from_id: &str,
        to_id: &str,
    ) -> Result<Option<EventId>> {
        let mut offset = 0;
        const PAGE_SIZE: usize = 1024;
        let relation_name = relation_kind_name(relation_kind);

        loop {
            let events = self
                .ledger
                .read_for_tenant(&self.context.tenant_id, offset, PAGE_SIZE)?;
            if events.is_empty() {
                return Ok(None);
            }

            for event in &events {
                if event.event_type != EventType::RelationAdded {
                    continue;
                }

                let same_relation = payload_value_matches(event, "relation", relation_name);
                let same_from = payload_value_matches(event, "from_id", from_id);
                let same_to = payload_value_matches(event, "to_id", to_id);
                if same_relation && same_from && same_to {
                    return Ok(event.event_id);
                }
            }

            if let Some(last_event_id) = events.last().and_then(|event| event.event_id) {
                offset = last_event_id;
            } else {
                return Ok(None);
            }
        }
    }

    fn scan_events(&self, predicate: impl Fn(&Event) -> bool) -> Result<bool> {
        let mut offset = 0;
        const PAGE_SIZE: usize = 1024;

        loop {
            let events = self
                .ledger
                .read_for_tenant(&self.context.tenant_id, offset, PAGE_SIZE)?;
            if events.is_empty() {
                return Ok(false);
            }

            for event in &events {
                if predicate(event) {
                    return Ok(true);
                }
            }

            if let Some(last_event_id) = events.last().and_then(|event| event.event_id) {
                offset = last_event_id;
            } else {
                return Ok(false);
            }
        }
    }
}

pub fn normalize_topic_key(input: &str) -> String {
    let transliterated = deunicode::deunicode(input);
    let mut normalized = String::with_capacity(transliterated.len());
    let mut last_was_separator = false;

    for character in transliterated.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            normalized.push(character);
            last_was_separator = false;
            continue;
        }

        if normalized.is_empty() || last_was_separator {
            continue;
        }

        normalized.push('-');
        last_was_separator = true;
    }

    while normalized.ends_with('-') {
        normalized.pop();
    }

    if normalized.len() > MAX_TOPIC_KEY_LEN {
        normalized.truncate(MAX_TOPIC_KEY_LEN);
        while normalized.ends_with('-') {
            normalized.pop();
        }
    }

    normalized
}

fn effective_topic_keys(requested: &[String], fallback: &[String]) -> Result<Vec<String>> {
    let normalized_requested = normalize_topic_keys(requested);
    let effective = if normalized_requested.is_empty() {
        fallback.to_vec()
    } else {
        normalized_requested
    };

    if effective.is_empty() {
        Err(CommandError::Validation(
            "topic_keys must contain at least one non-empty normalized key".to_owned(),
        )
        .into())
    } else {
        Ok(effective)
    }
}

fn normalize_topic_keys(topic_keys: &[String]) -> Vec<String> {
    topic_keys
        .iter()
        .map(|topic| normalize_topic_key(topic))
        .filter(|topic| !topic.is_empty())
        .collect()
}

fn effective_option_labels(
    new_title: &str,
    option_labels: &[String],
    chosen_option_label: Option<&str>,
) -> Result<Vec<String>> {
    let mut labels = Vec::with_capacity(option_labels.len().max(1));
    for option_label in option_labels {
        let trimmed = option_label.trim();
        if trimmed.is_empty() {
            return Err(CommandError::Validation(
                "option_labels must not contain empty values".to_owned(),
            )
            .into());
        }
        labels.push(trimmed.to_owned());
    }

    if labels.is_empty() {
        let fallback = chosen_option_label
            .map(str::trim)
            .filter(|label| !label.is_empty())
            .unwrap_or_else(|| new_title.trim());
        labels.push(fallback.to_owned());
    }

    Ok(labels)
}

fn deterministic_supersede_option_ids(
    actor_id: &str,
    old_decision_id: &str,
    new_title: &str,
    new_rationale: &str,
    option_labels: &[String],
) -> Vec<String> {
    option_labels
        .iter()
        .enumerate()
        .map(|(index, option_label)| {
            let stable_name =
                format!("{actor_id}\0{old_decision_id}\0{new_title}\0{new_rationale}\0{index}\0{option_label}");
            format!("option-{}", Uuid::new_v5(&Uuid::NAMESPACE_URL, stable_name.as_bytes()))
        })
        .collect()
}

fn decision_proposal_snapshot_from_event(event: &Event) -> Option<DecisionProposalSnapshot> {
    let event_id = event.event_id?;
    Some(DecisionProposalSnapshot {
        event_id,
        decision_id: payload_value_as_str(event, "decision_id")?.to_owned(),
        actor_id: event.actor_id.clone(),
        title: payload_value_as_str(event, "title")?.to_owned(),
        rationale: payload_value_as_str(event, "rationale")?.to_owned(),
        topic_keys: payload_string_list(event, "topic_keys"),
        option_ids: payload_string_list(event, "option_ids"),
        chosen_option_id: payload_value_as_str(event, "chosen_option_id").map(str::to_owned),
        hypothesis_ids: payload_string_list(event, "hypothesis_ids"),
        evidence_ids: payload_string_list(event, "evidence_ids"),
        project: payload_value_as_str(event, "project").map(str::to_owned),
        project_source: payload_value_as_str(event, "project_source")
            .and_then(ProjectSource::parse),
    })
}

fn payload_string_list(event: &Event, key: &str) -> Vec<String> {
    event
        .payload
        .get(key)
        .and_then(|value| value.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn payload_value_as_str<'a>(event: &'a Event, key: &str) -> Option<&'a str> {
    event.payload.get(key).and_then(|value| value.as_str())
}

fn payload_value_matches(event: &Event, key: &str, expected: &str) -> bool {
    payload_value_as_str(event, key).is_some_and(|actual| same_identifier(actual, expected))
}

fn same_identifier(left: &str, right: &str) -> bool {
    left.eq(right)
}

/// `quote` and `question` must be given together: a verbatim quote answering no stated
/// question is unreadable once the source conversation is gone (hivemind-zdsh.13).
fn require_quote_pairing(quote: Option<&str>, question: Option<&str>) -> Result<()> {
    if let Some(value) = quote {
        require_non_empty("quote", value)?;
    }
    if let Some(value) = question {
        require_non_empty("question", value)?;
    }
    if quote.is_some() != question.is_some() {
        return Err(CommandError::Validation(
            "quote and question must be given together — a verbatim answer needs the question it answers spelled out, not a bare reference like '1a' into an external list".to_owned(),
        )
        .into());
    }
    Ok(())
}

/// `rationale` must be readable on its own, without the source conversation — the write-time
/// half of Alex's finding-6 ruling (hivemind-763i, follow-up to hivemind-zdsh.13's optional
/// `quote`/`question`): a real minimum length, at least a full sentence's worth of words, and
/// — unless `quote`/`question` already carry the verbatim answer inline — no bare reference
/// into a numbered list that exists only in the source chat (the audited "verbatim: 1a, 2 this
/// seems weird..." shape). `has_quote_pair` must reflect a value already validated by
/// `require_quote_pairing` (quote and question both present or both absent).
fn require_readable_rationale(rationale: &str, has_quote_pair: bool) -> Result<()> {
    let trimmed = rationale.trim();

    if trimmed.chars().count() < MIN_RATIONALE_CHARS {
        return Err(CommandError::Validation(format!(
            "rationale must be at least {MIN_RATIONALE_CHARS} characters — a decision's why must be readable without the source conversation, not a stub"
        ))
        .into());
    }

    if trimmed.split_whitespace().count() < MIN_RATIONALE_WORDS {
        return Err(CommandError::Validation(format!(
            "rationale must be at least {MIN_RATIONALE_WORDS} words — write a full sentence, not a fragment"
        ))
        .into());
    }

    if !has_quote_pair {
        if let Some(reference) = find_bare_list_reference(trimmed) {
            return Err(CommandError::Validation(format!(
                "rationale references a bare list item ({reference:?}) that only makes sense against a list that isn't in the rationale itself — either write the rationale as self-contained prose, or pair quote with question to carry the verbatim words and the question they answer"
            ))
            .into());
        }
    }

    Ok(())
}

/// Finds the first enumerated-list-style token in `text`, e.g. `"1a"` or `"2. a"`: a run of
/// up to 3 digits, bounded by a non-word character (or the string edge) on both sides, that is
/// followed either directly, or — with a literal `.` and optional spaces — by a single
/// freestanding letter. Deliberately narrow: "TLS 1.2 support" and "24x7" do not match, since
/// this targets the shape of a citation into an external numbered list, not any digit near a
/// letter. A regex-class heuristic, not a parser — some legitimate prose (e.g. a sentence that
/// both ends in "N." and is immediately followed by a one-letter word) can still trip it; the
/// `quote`/`question` pair is the escape hatch for a rationale that legitimately needs to carry
/// one of these tokens.
fn find_bare_list_reference(text: &str) -> Option<String> {
    fn is_word_char(c: char) -> bool {
        c.is_alphanumeric() || c == '_'
    }

    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut i = 0;
    while i < len {
        if !chars[i].is_ascii_digit() {
            i += 1;
            continue;
        }

        let digit_start = i;
        let mut digit_end = i;
        while digit_end < len && chars[digit_end].is_ascii_digit() {
            digit_end += 1;
        }
        let before_ok = digit_start == 0 || !is_word_char(chars[digit_start - 1]);
        i = digit_end;

        if digit_end - digit_start > 3 || !before_ok {
            continue;
        }

        // Case A: digit run directly followed by one freestanding letter, e.g. "1a".
        if digit_end < len && chars[digit_end].is_ascii_alphabetic() {
            let letter_end = digit_end + 1;
            if letter_end >= len || !is_word_char(chars[letter_end]) {
                return Some(chars[digit_start..letter_end].iter().collect());
            }
        }

        // Case B: digit run, a period, optional spaces, then one freestanding letter, e.g. "2. a".
        if digit_end < len && chars[digit_end] == '.' {
            let mut letter_start = digit_end + 1;
            while letter_start < len && chars[letter_start] == ' ' {
                letter_start += 1;
            }
            if letter_start < len && chars[letter_start].is_ascii_alphabetic() {
                let letter_end = letter_start + 1;
                if letter_end >= len || !is_word_char(chars[letter_end]) {
                    return Some(chars[digit_start..letter_end].iter().collect());
                }
            }
        }
    }
    None
}

/// Expressed confidence is a fixed, decider-stated vocabulary — never a system-computed
/// score (see `DecisionProposalInput::expressed_confidence`).
fn require_valid_expressed_confidence(value: Option<&str>) -> Result<()> {
    match value {
        None | Some("low") | Some("medium") | Some("high") => Ok(()),
        Some(other) => Err(CommandError::Validation(format!(
            "expressed_confidence must be low, medium, or high (got: {other})"
        ))
        .into()),
    }
}

/// A decision follows from *other* decisions; naming itself as its own premise is a
/// self-loop in the premise graph, never a grounding.
fn require_not_own_premise(decision_id: &str, premise_id: &str) -> Result<()> {
    if same_identifier(decision_id, premise_id) {
        return Err(
            CommandError::Validation("a decision cannot be its own premise".to_owned()).into(),
        );
    }
    Ok(())
}

const fn relation_kind_name(relation_kind: RelationKind) -> &'static str {
    match relation_kind {
        RelationKind::BasedOn => "BASED_ON",
        RelationKind::HasOption => "HAS_OPTION",
        RelationKind::Chose => "CHOSE",
        RelationKind::Assumes => "ASSUMES",
        RelationKind::Supports => "SUPPORTS",
        RelationKind::Refutes => "REFUTES",
        RelationKind::SameAs => "SAME_AS",
        RelationKind::FollowsFrom => "FOLLOWS_FROM",
    }
}

fn require_optional_non_empty(field: &'static str, value: Option<&str>) -> Result<()> {
    if value.is_some_and(|value| value.trim().is_empty()) {
        Err(CommandError::Validation(format!("{field} must not be empty")).into())
    } else {
        Ok(())
    }
}

/// Handle format: lowercase letters, digits, dashes; 2-40 chars; the `personal:` prefix is
/// reserved for the actor-derived personal project address and is never typed or registered.
fn validate_project_handle(handle: &str) -> Result<()> {
    require_non_empty("handle", handle)?;

    if handle.starts_with(PERSONAL_PROJECT_HANDLE_PREFIX) {
        return Err(CommandError::Validation(format!(
            "project handle must not use the reserved \"{PERSONAL_PROJECT_HANDLE_PREFIX}\" prefix: {handle}"
        ))
        .into());
    }

    let len = handle.chars().count();
    if !(MIN_PROJECT_HANDLE_LEN..=MAX_PROJECT_HANDLE_LEN).contains(&len) {
        return Err(CommandError::Validation(format!(
            "project handle must be {MIN_PROJECT_HANDLE_LEN}-{MAX_PROJECT_HANDLE_LEN} characters: {handle}"
        ))
        .into());
    }

    if !handle
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(CommandError::Validation(format!(
            "project handle must contain only lowercase letters, digits, and dashes: {handle}"
        ))
        .into());
    }

    Ok(())
}

/// The personal project address for an actor (Alex, choice 3a): the actor id with its
/// session component removed, then prefixed with `personal:`. `agent:<tool>:<session>`
/// drops the session and becomes `personal:agent:<tool>`; `human:<id>` has no session
/// component to remove and becomes `personal:human:<id>`. The session itself is never
/// lost — it stays on the event as provenance (`agent_session`/`source_ref`), reused
/// rather than duplicated onto a second field. Any other actor id shape (no documented
/// session convention) is prefixed as-is rather than guessed at.
pub fn personal_project_handle(actor_id: &str) -> String {
    let base = match actor_id
        .strip_prefix("agent:")
        .and_then(|rest| rest.split_once(':'))
    {
        Some((tool, _session)) => format!("agent:{tool}"),
        None => actor_id.to_owned(),
    };
    format!("{PERSONAL_PROJECT_HANDLE_PREFIX}{base}")
}

/// A decision title is a name, not a summary: at most `MAX_TITLE_LEN` characters, one
/// sentence, no numbered list. Longer reasoning belongs in `rationale`. Rejects rather than
/// truncates, so a run-on never silently loses its tail (hivemind-zdsh.12).
fn validate_title(field: &'static str, title: &str) -> Result<()> {
    require_non_empty(field, title)?;

    let trimmed = title.trim();
    let char_count = trimmed.chars().count();
    if char_count > MAX_TITLE_LEN {
        return Err(CommandError::Validation(format!(
            "{field} must be at most {MAX_TITLE_LEN} characters (got {char_count}); it is a name, not a summary — move the rest to rationale"
        ))
        .into());
    }

    let terminal_marks = count_terminal_punctuation(trimmed);
    if terminal_marks > 1 {
        return Err(CommandError::Validation(format!(
            "{field} must be a single sentence: found {terminal_marks} terminal punctuation marks ('.', '!', '?'); split into title + rationale"
        ))
        .into());
    }

    if looks_like_numbered_list(trimmed) {
        return Err(CommandError::Validation(format!(
            "{field} must not be a numbered list (found two or more '1.'/'2)'-style markers); move list items to rationale"
        ))
        .into());
    }

    Ok(())
}

/// Counts '.'/'!'/'?' that sit at a sentence boundary (end of string, or followed by
/// whitespace) — so decimal/version punctuation like "v1.2.3" does not count as multiple
/// sentences.
fn count_terminal_punctuation(text: &str) -> usize {
    let chars: Vec<char> = text.chars().collect();
    chars
        .iter()
        .enumerate()
        .filter(|(i, &c)| {
            matches!(c, '.' | '!' | '?') && chars.get(i + 1).is_none_or(|next| next.is_whitespace())
        })
        .count()
}

/// True when the text contains two or more whitespace-delimited numbered-list markers, e.g.
/// "1. Do X 2. Do Y" — a title enumerating several clauses instead of naming one decision.
fn looks_like_numbered_list(text: &str) -> bool {
    let markers = text
        .split_whitespace()
        .filter(|token| {
            let stripped = token.trim_end_matches(['.', ')', ':']);
            !stripped.is_empty()
                && stripped != *token
                && stripped.bytes().all(|b| b.is_ascii_digit())
        })
        .count();
    markers >= 2
}

fn generate_entity_id(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4())
}

/// Option ids are opaque (hivemind-zdsh.10): the id used to be a slug of the whole label
/// (`option-<slugified-label>-<uuid>`), which produced 90+ character ids and made the id and
/// the label the same piece of data wearing two hats. The label is the only place option text
/// lives now; the id is just a handle, matching `generate_entity_id` for every other node kind.
fn generate_option_id() -> String {
    generate_entity_id("option")
}

/// Upper bound on a captured option label (hivemind-zdsh.10 FIX). Labels are meant to be one
/// short human-readable name for the option, not a summary paragraph — everything longer than
/// this is a sign the caller bundled several answers or a rationale into one option.
const MAX_OPTION_LABEL_LEN: usize = 80;

/// Content quality gate for a captured option label, run once at `record_option_with_id` time —
/// the single choke point every capture path (`capture_decision` MCP/API tool, `hivemind emit
/// decision.proposed`, and `supersede`) routes an option label through before it ever reaches
/// the ledger. Three failure modes observed in the wild (hivemind-zdsh.10 audit, decisions
/// ca4da326 / 502c4375 / others):
///   1. The label re-encodes the choice itself ("...-chosen-...") — `CHOSE` is the only
///      representation of which option was picked; a label must not duplicate that.
///   2. A "bucket" option that stands in for several distinct rejected options at once
///      ("other rejected options") instead of one real option per candidate.
///   3. Several numbered answers bundled into a single option label (e.g. "1a-2-3a-4a-5a-6a").
///
/// This only gates *new* captures — replay of historical events never calls it, so ledger
/// history written before this check existed keeps replaying unchanged.
fn validate_option_label(label: &str) -> Result<()> {
    let trimmed = label.trim();
    if trimmed.chars().count() > MAX_OPTION_LABEL_LEN {
        return Err(CommandError::Validation(format!(
            "option label exceeds {MAX_OPTION_LABEL_LEN} characters ({} chars): {trimmed:?} — \
             keep the label to one short name for the option; put detail in the description",
            trimmed.chars().count()
        ))
        .into());
    }

    let lower = trimmed.to_ascii_lowercase();
    if lower.contains("chosen") {
        return Err(CommandError::Validation(format!(
            "option label must not encode the decision outcome (found 'chosen' in {trimmed:?}); \
             the CHOSE edge (chosen_option_id) is the only representation of which option was picked"
        ))
        .into());
    }
    if lower.contains("rejected options") {
        return Err(CommandError::Validation(format!(
            "option label reads as a bucket of rejected options ({trimmed:?}); capture each \
             rejected option as its own option instead of grouping them into one"
        ))
        .into());
    }
    if bundles_numbered_answers(&lower) {
        return Err(CommandError::Validation(format!(
            "option label appears to bundle multiple numbered answers into one option \
             ({trimmed:?}); capture each answer as its own option"
        ))
        .into());
    }

    Ok(())
}

/// Heuristic for "several numbered answers packed into one label" (e.g.
/// "alex-s-answers-1a-2-common-parent-3a-4a-5a-6a-plus-child-as-evid"): tokenize on non-
/// alphanumeric boundaries and count tokens that look like a short numbered-answer reference
/// (1-2 digits optionally followed by a single letter, e.g. "1a", "2", "6a"). Three or more
/// such tokens in one label is treated as bundling rather than a single legitimate reference
/// (a lone "phase 1" or a version like "v2" doesn't trip this; a 4-digit run like a year
/// doesn't either).
fn bundles_numbered_answers(lower_label: &str) -> bool {
    lower_label
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|token| is_numbered_answer_token(token))
        .count()
        >= 3
}

fn is_numbered_answer_token(token: &str) -> bool {
    let mut chars = token.chars();
    let digit_count = chars.by_ref().take_while(char::is_ascii_digit).count();
    if digit_count == 0 || digit_count > 2 {
        return false;
    }
    match chars.as_str().chars().collect::<Vec<_>>().as_slice() {
        [] => true,
        [c] => c.is_ascii_lowercase(),
        _ => false,
    }
}

fn repeat_uuid(count: usize) -> Vec<Uuid> {
    (0..count).map(|_| Uuid::new_v4()).collect()
}

#[cfg(test)]
mod tests;
