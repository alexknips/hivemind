//! Projector trait and graph types: replays ledger events into a live in-memory graph view.

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap};

use serde::Serialize;

use crate::commands::personal_project_handle;
use crate::error::ProjectorError;
use crate::events::{
    self, BlockerReportedPayload, BlockerResolvedPayload, CaptureItem, DecisionMovedPayload,
    DecisionProposedPayload, DecisionRequestedPayload, DecisionScoredPayload, Event, EventId,
    EventPayload, EvidenceRecordedPayload, HypothesisRecordedPayload, IngestBatchClassifiedPayload,
    NotificationAcknowledgedPayload, NotificationSentPayload, ProjectAnchorKind,
    ProjectAnchorPayload, ProjectLinkKind, ProjectRegisteredPayload, ProjectSource,
    RelationKind as EventRelationKind, TenantId,
};
use crate::ledger::EventLedger;
use crate::Result;

pub mod arrow;
#[cfg(feature = "graph-kuzu")]
pub mod kuzu;
pub mod memory;
#[cfg(feature = "shared-backend-postgres")]
pub mod postgres;

pub type GraphProperties = BTreeMap<String, GraphValue>;
pub type GraphParams = BTreeMap<String, GraphValue>;
pub type GraphRow = BTreeMap<String, GraphValue>;

type ProjectedGraph = (
    Vec<(NodeKind, String, String)>,
    Vec<(RelationKind, String, String)>,
);

#[derive(Clone, Debug, PartialEq)]
pub enum GraphValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    StringList(Vec<String>),
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Decision,
    DecisionRequest,
    Actor,
    Blocker,
    Evidence,
    Notification,
    Option,
    Hypothesis,
    /// Shared project. Personal projects never get a node — they're derived from the
    /// actor id at query time, never registered (see `commands::register_project`).
    Project,
}

impl NodeKind {
    pub const ALL: [Self; 9] = [
        Self::Decision,
        Self::DecisionRequest,
        Self::Actor,
        Self::Blocker,
        Self::Evidence,
        Self::Notification,
        Self::Option,
        Self::Hypothesis,
        Self::Project,
    ];

    pub const fn table_name(self) -> &'static str {
        match self {
            Self::Decision => "Decision",
            Self::DecisionRequest => "DecisionRequest",
            Self::Actor => "Actor",
            Self::Blocker => "Blocker",
            Self::Evidence => "Evidence",
            Self::Notification => "Notification",
            Self::Option => "Option",
            Self::Hypothesis => "Hypothesis",
            Self::Project => "Project",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    ProposedBy,
    DecisionRequestedBy,
    DecisionRequestForDecision,
    DecisionRequestRequiredOwner,
    AcceptedBy,
    RejectedBy,
    /// Actor who proposed a `DecisionRequest` that also has an `accepted_by`/`rejected_by`
    /// position on record (contested ask). Distinct table from `ProposedBy` because Kuzu rel
    /// tables are bound to a fixed node-kind pair — see the graph reference doc for why the
    /// proposer/accepter/rejecter roles split into Decision- and DecisionRequest-scoped
    /// variants instead of widening the existing tables.
    RequestProposedBy,
    /// Actor who accepted a `DecisionRequest`. See `RequestProposedBy`.
    RequestAcceptedBy,
    /// Actor who rejected a `DecisionRequest`. See `RequestProposedBy`.
    RequestRejectedBy,
    Supersedes,
    BlockedActor,
    BlockerForDecision,
    BlockerRequiredOwner,
    NotificationForBlocker,
    NotificationRecipient,
    BasedOn,
    HasOption,
    Chose,
    PremisedOn,
    PremisedOnDirect,
    Supports,
    Refutes,
    SameAs,
    /// Actor that participated in the session that produced this decision.
    ParticipatedBy,
    /// Actor that initiated the session that produced this decision.
    InitiatedBy,
    /// `from` project is part of `to` project (at most one active parent per project).
    PartOf,
    /// `from` project depends on `to` project (one hop, not transitive by default).
    DependsOn,
    /// `from` decision follows from `to` decision — a premise in the broad sense.
    FollowsFrom,
}

impl RelationKind {
    pub const ALL: [Self; 28] = [
        Self::ProposedBy,
        Self::DecisionRequestedBy,
        Self::DecisionRequestForDecision,
        Self::DecisionRequestRequiredOwner,
        Self::AcceptedBy,
        Self::RejectedBy,
        Self::RequestProposedBy,
        Self::RequestAcceptedBy,
        Self::RequestRejectedBy,
        Self::Supersedes,
        Self::BlockedActor,
        Self::BlockerForDecision,
        Self::BlockerRequiredOwner,
        Self::NotificationForBlocker,
        Self::NotificationRecipient,
        Self::BasedOn,
        Self::HasOption,
        Self::Chose,
        Self::PremisedOn,
        Self::PremisedOnDirect,
        Self::Supports,
        Self::Refutes,
        Self::SameAs,
        Self::ParticipatedBy,
        Self::InitiatedBy,
        Self::PartOf,
        Self::DependsOn,
        Self::FollowsFrom,
    ];

    pub const fn table_name(self) -> &'static str {
        match self {
            Self::ProposedBy => "PROPOSED_BY",
            Self::DecisionRequestedBy => "DECISION_REQUESTED_BY",
            Self::DecisionRequestForDecision => "DECISION_REQUEST_FOR_DECISION",
            Self::DecisionRequestRequiredOwner => "DECISION_REQUEST_REQUIRED_OWNER",
            Self::AcceptedBy => "ACCEPTED_BY",
            Self::RejectedBy => "REJECTED_BY",
            Self::RequestProposedBy => "REQUEST_PROPOSED_BY",
            Self::RequestAcceptedBy => "REQUEST_ACCEPTED_BY",
            Self::RequestRejectedBy => "REQUEST_REJECTED_BY",
            Self::Supersedes => "SUPERSEDES",
            Self::BlockedActor => "BLOCKED_ACTOR",
            Self::BlockerForDecision => "BLOCKER_FOR_DECISION",
            Self::BlockerRequiredOwner => "BLOCKER_REQUIRED_OWNER",
            Self::NotificationForBlocker => "NOTIFICATION_FOR_BLOCKER",
            Self::NotificationRecipient => "NOTIFICATION_RECIPIENT",
            Self::BasedOn => "BASED_ON",
            Self::HasOption => "HAS_OPTION",
            Self::Chose => "CHOSE",
            Self::PremisedOn => "PREMISED_ON",
            Self::PremisedOnDirect => "PREMISED_ON_DIRECT",
            Self::Supports => "SUPPORTS",
            Self::Refutes => "REFUTES",
            Self::SameAs => "SAME_AS",
            Self::ParticipatedBy => "PARTICIPATED_BY",
            Self::InitiatedBy => "INITIATED_BY",
            Self::PartOf => "PART_OF",
            Self::DependsOn => "DEPENDS_ON",
            Self::FollowsFrom => "FOLLOWS_FROM",
        }
    }

    pub const fn endpoints(self) -> (NodeKind, NodeKind) {
        match self {
            Self::ProposedBy | Self::AcceptedBy | Self::RejectedBy => {
                (NodeKind::Decision, NodeKind::Actor)
            }
            Self::DecisionRequestedBy | Self::DecisionRequestRequiredOwner => {
                (NodeKind::DecisionRequest, NodeKind::Actor)
            }
            Self::RequestProposedBy | Self::RequestAcceptedBy | Self::RequestRejectedBy => {
                (NodeKind::DecisionRequest, NodeKind::Actor)
            }
            Self::DecisionRequestForDecision => (NodeKind::DecisionRequest, NodeKind::Decision),
            Self::Supersedes => (NodeKind::Decision, NodeKind::Decision),
            Self::BlockedActor | Self::BlockerRequiredOwner => (NodeKind::Blocker, NodeKind::Actor),
            Self::BlockerForDecision => (NodeKind::Blocker, NodeKind::Decision),
            Self::NotificationForBlocker => (NodeKind::Notification, NodeKind::Blocker),
            Self::NotificationRecipient => (NodeKind::Notification, NodeKind::Actor),
            Self::BasedOn => (NodeKind::Decision, NodeKind::Evidence),
            Self::HasOption | Self::Chose => (NodeKind::Decision, NodeKind::Option),
            Self::PremisedOn => (NodeKind::Option, NodeKind::Hypothesis),
            Self::PremisedOnDirect => (NodeKind::Decision, NodeKind::Hypothesis),
            Self::Supports | Self::Refutes => (NodeKind::Evidence, NodeKind::Hypothesis),
            Self::SameAs => (NodeKind::Decision, NodeKind::Decision),
            Self::ParticipatedBy | Self::InitiatedBy => (NodeKind::Decision, NodeKind::Actor),
            Self::PartOf | Self::DependsOn => (NodeKind::Project, NodeKind::Project),
            Self::FollowsFrom => (NodeKind::Decision, NodeKind::Decision),
        }
    }
}

pub trait GraphView {
    fn upsert_node(&self, kind: NodeKind, id: &str, properties: &GraphProperties) -> Result<()>;

    fn upsert_edge(
        &self,
        kind: RelationKind,
        from_id: &str,
        to_id: &str,
        properties: &GraphProperties,
    ) -> Result<()>;

    fn query(&self, cypher: &str, params: &GraphParams) -> Result<Vec<GraphRow>>;

    fn wipe(&self) -> Result<()>;
}

pub fn project_event(graph: &impl GraphView, event: &Event) -> Result<()> {
    let payload = events::validate(event).map_err(projector_error)?;
    let event_origin = event_origin(event)?;
    let origin_properties = origin_properties(event, event_origin);

    upsert_actor(graph, &event.actor_id, &origin_properties)?;

    match payload {
        EventPayload::DecisionProposed(payload) => project_decision_proposed(
            graph,
            &event.actor_id,
            &payload,
            &origin_properties,
            event_timestamp(event),
        )?,
        EventPayload::DecisionRequested(payload) => {
            project_decision_requested(graph, event, &payload, &origin_properties)?
        }
        EventPayload::DecisionAccepted(payload) => {
            graph.upsert_edge(
                RelationKind::AcceptedBy,
                &payload.decision_id,
                &event.actor_id,
                &origin_properties,
            )?;
            // Delegation marker (hivemind-zdsh.6): a property on the Decision node, not on
            // the ACCEPTED_BY edge and not a new node/edge kind — orthogonal to the
            // authorship/review shapes derived from those edges. Only the marker is
            // upserted, so the node keeps the proposal's own event_origin/source/tenant.
            if let Some(delegated_by) = payload.delegated_by {
                graph.upsert_node(
                    NodeKind::Decision,
                    &payload.decision_id,
                    &GraphProperties::from([(
                        "delegated_by".to_owned(),
                        GraphValue::String(delegated_by),
                    )]),
                )?;
            }
        }
        EventPayload::DecisionRejected(payload) => graph.upsert_edge(
            RelationKind::RejectedBy,
            &payload.decision_id,
            &event.actor_id,
            &origin_properties,
        )?,
        EventPayload::DecisionSuperseded(payload) => graph.upsert_edge(
            RelationKind::Supersedes,
            &payload.new_decision_id,
            &payload.old_decision_id,
            &origin_properties,
        )?,
        EventPayload::EvidenceRecorded(payload) => {
            project_evidence_recorded(graph, &payload, &origin_properties, event_timestamp(event))?
        }
        EventPayload::HypothesisRecorded(payload) => {
            project_hypothesis_recorded(graph, &payload, &origin_properties)?
        }
        EventPayload::RelationAdded(payload) => {
            let kind = if payload.relation == EventRelationKind::Assumes {
                // Route based on from_id prefix: option-prefix → PREMISED_ON (Option→Hypothesis),
                // decision-prefix or legacy → PREMISED_ON_DIRECT (Decision→Hypothesis).
                if payload.from_id.starts_with("option-") || payload.from_id.starts_with("option:")
                {
                    RelationKind::PremisedOn
                } else {
                    RelationKind::PremisedOnDirect
                }
            } else {
                relation_kind(payload.relation)
            };
            if is_grounding_relation(kind) {
                let properties = grounding_edge_properties(
                    &origin_properties,
                    &event.actor_id,
                    event_timestamp(event),
                    event
                        .causation_event_id
                        .and_then(|causation| i64::try_from(causation).ok()),
                );
                graph.upsert_edge(kind, &payload.from_id, &payload.to_id, &properties)?;
            } else {
                graph.upsert_edge(kind, &payload.from_id, &payload.to_id, &origin_properties)?;
            }
        }
        EventPayload::BlockerReported(payload) => {
            project_blocker_reported(graph, event, event_origin, &payload, &origin_properties)?
        }
        EventPayload::BlockerResolved(payload) => {
            project_blocker_resolved(graph, event, event_origin, &payload, &origin_properties)?
        }
        EventPayload::NotificationSent(payload) => {
            project_notification_sent(graph, event, &payload, &origin_properties)?
        }
        EventPayload::NotificationAcknowledged(payload) => {
            project_notification_acknowledged(graph, &payload, &origin_properties)?
        }
        EventPayload::IngestBatchReceived(_) => {
            // Raw transcript batches are ledger-only; they do not project to the graph.
        }
        EventPayload::IngestBatchClassified(payload) => {
            project_ingest_batch_classified(graph, event_origin, &payload, &origin_properties)?
        }
        EventPayload::DecisionScored(payload) => {
            project_decision_scored(graph, &payload, &origin_properties)?
        }
        EventPayload::RelationRemoved(_) => {
            // GraphView has no remove_edge; retraction is recorded in the ledger only
        }
        EventPayload::DecisionMetadataDerived(_) => {
            // Derived metadata is stored in the ledger only; graph projection deferred to layer 3
        }
        EventPayload::DecisionMoved(payload) => {
            project_decision_moved(graph, &payload)?;
        }
        EventPayload::ProjectRegistered(payload) => {
            project_project_registered(graph, &payload, &origin_properties)?
        }
        EventPayload::ProjectLinked(payload) => {
            let kind = project_link_relation_kind(payload.kind);
            graph.upsert_edge(kind, &payload.from, &payload.to, &origin_properties)?;
        }
        EventPayload::ProjectUnlinked(_) => {
            // GraphView has no remove_edge; retraction is recorded in the ledger only
        }
        EventPayload::ProjectAnchored(payload) => {
            project_project_anchored(graph, &payload, &origin_properties)?
        }
        EventPayload::ProjectUnanchored(payload) => {
            project_project_unanchored(graph, &payload, &origin_properties)?
        }
    }

    Ok(())
}

pub fn project_from_ledger(
    ledger: &impl EventLedger,
    graph: &impl GraphView,
    offset: EventId,
) -> Result<()> {
    ledger.replay_from(offset, &mut |event| project_event(graph, event))
}

pub fn project_from_ledger_for_tenant(
    ledger: &impl EventLedger,
    tenant_id: &TenantId,
    graph: &impl GraphView,
    offset: EventId,
) -> Result<()> {
    ledger.replay_from_for_tenant(tenant_id, offset, &mut |event| project_event(graph, event))
}

pub fn rebuild_graph(ledger: &impl EventLedger, graph: &impl GraphView) -> Result<()> {
    graph.wipe()?;
    project_from_ledger(ledger, graph, 0)
}

pub fn rebuild_graph_for_tenant(
    ledger: &impl EventLedger,
    tenant_id: &TenantId,
    graph: &impl GraphView,
) -> Result<()> {
    graph.wipe()?;
    project_from_ledger_for_tenant(ledger, tenant_id, graph, 0)
}

/// Project a slice of captures in-memory and return the resulting graph as
/// flat vectors for evaluation. Each item in `id_captures` is
/// `(stable_node_id, &CaptureItem)` — the caller assigns IDs. Returns
/// `(nodes, edges)` where nodes are `(NodeKind, node_id, text)` and edges
/// are `(RelationKind, from_id, to_id)`. Notification nodes are excluded.
pub fn project_captures_in_memory(id_captures: &[(&str, &CaptureItem)]) -> Result<ProjectedGraph> {
    let graph = memory::MemoryGraph::default();
    let (resolved, _stats) = resolve_batch_local_references(id_captures);
    for (node_id, capture) in &resolved {
        project_capture(&graph, capture, node_id, &GraphProperties::default())?;
    }
    let (nodes_map, edges) = graph.nodes_and_edges()?;
    let nodes = nodes_map
        .into_iter()
        .filter_map(|((kind, id), props)| {
            if kind == NodeKind::Notification {
                return None;
            }
            let text = capture_node_text(kind, &id, &props);
            Some((kind, id, text))
        })
        .collect();
    Ok((nodes, edges))
}

/// Batch-level readout from [`resolve_batch_local_references`]. There is no
/// metrics system in this codebase (checked: no `metrics`/`prometheus` crate
/// anywhere) — `tracing` is the existing observability surface, so this
/// struct is reported via a `tracing::debug!` at the end of the same
/// function rather than inventing a counters framework. It's also returned
/// so callers (and tests) can read the numbers directly instead of scraping
/// logs.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct BatchReferenceStats {
    /// Relational-id values that matched a sibling's title in this batch and
    /// were rewritten to that sibling's real node id. This is the direct
    /// readout on whether the title-as-local-key convention (hivemind-o4l6)
    /// is actually firing.
    resolved: usize,
    /// Non-empty relational-id values that did NOT match any sibling title
    /// and fell through unchanged (the pre-existing verbatim-id pass-through
    /// behavior). This bucket conflates two very different cases — a
    /// legitimate reference to a real id from an earlier batch, and a
    /// near-miss title the model failed to reproduce exactly — so treat it
    /// as a ceiling on the near-miss rate, not a direct measurement of it.
    unresolved: usize,
}

fn resolve_local_reference(
    title_index: &HashMap<&str, &str>,
    stats: &mut BatchReferenceStats,
    node_id: &str,
    raw: &str,
) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return raw.to_owned();
    }
    match title_index.get(trimmed) {
        Some(target) if *target != node_id => {
            stats.resolved += 1;
            (*target).to_owned()
        }
        _ => {
            stats.unresolved += 1;
            raw.to_owned()
        }
    }
}

/// Resolve same-batch cross-references in a slice of captures.
///
/// Real HiveMind node ids are assigned post-hoc (ledger offset + array
/// index for the production path, or a caller-chosen id for the in-memory
/// eval path) — the classifier can never know a sibling capture's real id
/// before generating output. So the classifier prompt allows a relational-id
/// field (`evidence_ids`, `premised_on_ids`, `supersedes_id`, `supports_ids`,
/// `refutes_ids`) to name a sibling capture in the *same* response by its
/// exact `title` instead. This resolves those title references to the
/// sibling's real node id before projection; any value that doesn't match a
/// sibling's title is left untouched (the existing verbatim-id pass-through
/// behavior, via `ensure_node_reference`, is unchanged).
///
/// When two captures in the batch share a title, the title index keeps the
/// first one (deterministic `or_insert`-style first-wins) and the second
/// becomes unreferenceable by title. That's safe — it can never corrupt an
/// edge — but it's also completely silent by default, so a collision logs a
/// `tracing::warn!` naming the title and both node ids.
fn resolve_batch_local_references(
    id_captures: &[(&str, &CaptureItem)],
) -> (Vec<(String, CaptureItem)>, BatchReferenceStats) {
    let mut title_index: HashMap<&str, &str> = HashMap::new();
    for (node_id, capture) in id_captures {
        let title = capture.title.trim();
        match title_index.entry(title) {
            Entry::Occupied(existing) => {
                tracing::warn!(
                    target: "hivemind::projector",
                    title,
                    first_node_id = *existing.get(),
                    duplicate_node_id = *node_id,
                    "duplicate capture title within batch: first-wins resolution keeps \
                     referencing the first capture; the duplicate is unreferenceable by title"
                );
            }
            Entry::Vacant(slot) => {
                slot.insert(*node_id);
            }
        }
    }

    let mut stats = BatchReferenceStats::default();
    let resolved = id_captures
        .iter()
        .map(|(node_id, capture)| {
            let mut resolved = (*capture).clone();
            resolved.supersedes_id = resolved
                .supersedes_id
                .as_deref()
                .map(|raw| resolve_local_reference(&title_index, &mut stats, node_id, raw));
            resolved.premised_on_ids = resolved
                .premised_on_ids
                .iter()
                .map(|s| resolve_local_reference(&title_index, &mut stats, node_id, s))
                .collect();
            resolved.supports_ids = resolved
                .supports_ids
                .iter()
                .map(|s| resolve_local_reference(&title_index, &mut stats, node_id, s))
                .collect();
            resolved.refutes_ids = resolved
                .refutes_ids
                .iter()
                .map(|s| resolve_local_reference(&title_index, &mut stats, node_id, s))
                .collect();
            resolved.evidence_ids = resolved
                .evidence_ids
                .iter()
                .map(|s| resolve_local_reference(&title_index, &mut stats, node_id, s))
                .collect();
            ((*node_id).to_owned(), resolved)
        })
        .collect();

    if stats.resolved > 0 || stats.unresolved > 0 {
        tracing::debug!(
            target: "hivemind::projector",
            resolved = stats.resolved,
            unresolved = stats.unresolved,
            "within-batch title reference resolution stats"
        );
    }

    (resolved, stats)
}

fn capture_node_text(kind: NodeKind, id: &str, props: &GraphProperties) -> String {
    let key = match kind {
        NodeKind::Decision => "title",
        NodeKind::Evidence => "content",
        NodeKind::Hypothesis => "statement",
        NodeKind::Blocker | NodeKind::DecisionRequest => "reason",
        NodeKind::Option => "label",
        // Actor nodes have no text property; use the ID as a scoring proxy so
        // gold_as_captures() IDs (e.g. "mia") match the produced Actor node ID.
        NodeKind::Actor => return id.to_owned(),
        // No capture kind ever produces a Project node (see `project_capture`'s match);
        // this arm exists only for exhaustiveness.
        NodeKind::Notification | NodeKind::Project => return String::new(),
    };
    match props.get(key) {
        Some(GraphValue::String(s)) => s.clone(),
        _ => String::new(),
    }
}

/// Derives a readable label from a legacy option id for events that predate the
/// `option_labels` field (hivemind-zdsh.10 migration). The pre-opaque-id generator
/// (`commands::generate_option_id`, before this bead) built ids as
/// `option-<slugified-label>-<uuid>`, so the label text is recoverable by stripping the
/// `option-` prefix and the trailing UUID and turning the remaining slug into words.
///
/// Deliberately conservative: requires *both* the exact `option-` prefix *and* a real trailing
/// UUID to actually be present and stripped, not just "whatever's left after removing a
/// prefix". Custom option-id schemes (e.g. a colon-namespaced `org:launch:option:public-now`
/// used by some fixtures, or a short deterministic test id like `option-001-b`) don't match
/// that specific old shape and must return `None` unchanged rather than have an arbitrary
/// hyphen swapped for a space — that would invent a label distinction that was never
/// captured, not migrate one that was (AGENTS.md §6: no invented confidence).
fn derive_legacy_option_label(option_id: &str) -> Option<String> {
    let without_prefix = option_id.strip_prefix("option-")?;
    let without_uuid = strip_trailing_uuid(without_prefix)?;
    let derived = without_uuid
        .split(['-', '_'])
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if derived.is_empty() {
        None
    } else {
        Some(derived)
    }
}

/// Strips a trailing canonical-form UUID (36 chars: 8-4-4-4-12 hex digits) and its separating
/// hyphen from `s`, returning `None` when `s` doesn't end in one.
fn strip_trailing_uuid(s: &str) -> Option<&str> {
    if s.len() <= 37 {
        return None;
    }
    let split_at = s.len() - 37;
    if s.as_bytes().get(split_at) != Some(&b'-') {
        return None;
    }
    let tail = s.get(split_at + 1..)?;
    let head = s.get(..split_at)?;
    is_uuid_like(tail).then_some(head)
}

fn is_uuid_like(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(i, &b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_hexdigit(),
        })
}

fn option_node_id(decision_id: &str, label: &str) -> String {
    let slug: String = label
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("{decision_id}:opt:{slug}")
}

fn upsert_actor(
    graph: &impl GraphView,
    actor_id: &str,
    properties: &GraphProperties,
) -> Result<()> {
    let mut props = properties.clone();
    props.insert(
        "kind".to_owned(),
        GraphValue::String(actor_kind(actor_id).to_owned()),
    );
    graph.upsert_node(NodeKind::Actor, actor_id, &props)
}

fn actor_kind(actor_id: &str) -> &'static str {
    if actor_id.starts_with("human:") {
        "human"
    } else if actor_id.starts_with("agent:") {
        "agent"
    } else {
        "unknown"
    }
}

fn ensure_node_reference(
    graph: &impl GraphView,
    kind: NodeKind,
    id: &str,
    properties: &GraphProperties,
) -> Result<()> {
    let table = kind.table_name();
    let rows = graph.query(
        &format!("MATCH (node:`{table}` {{id: $id}}) RETURN node.id AS id LIMIT 1;"),
        &GraphParams::from([("id".to_owned(), GraphValue::String(id.to_owned()))]),
    )?;
    if rows.is_empty() {
        graph.upsert_node(kind, id, properties)?;
    }
    Ok(())
}

/// Writes properties onto a node that an earlier event created, without moving the node's
/// `event_origin`. An event that only annotates a node (a score, a blocker resolution, a
/// notification acknowledgement, a project anchor) is not the event that created it, and
/// `event_origin` is the ledger offset that did (AGENTS.md section 4). Arrows are oriented by
/// that offset (see `arrow`), so letting a later annotation move it would flip arrows after
/// unrelated events. A node the annotation names before any event created it gets a
/// placeholder carrying this event's origin, exactly as `ensure_node_reference` does.
fn annotate_node(
    graph: &impl GraphView,
    kind: NodeKind,
    id: &str,
    origin_properties: &GraphProperties,
    properties: &GraphProperties,
) -> Result<()> {
    ensure_node_reference(graph, kind, id, origin_properties)?;
    graph.upsert_node(kind, id, properties)
}

fn optional_string_value(value: Option<&str>) -> GraphValue {
    value.map_or(GraphValue::Null, |value| {
        GraphValue::String(value.to_owned())
    })
}

fn origin_properties(event: &Event, event_origin: i64) -> GraphProperties {
    let mut properties = GraphProperties::from([
        (
            "tenant_id".to_owned(),
            GraphValue::String(event.tenant_id.as_str().to_owned()),
        ),
        ("event_origin".to_owned(), GraphValue::Int(event_origin)),
        (
            "source".to_owned(),
            GraphValue::String(event.source.as_str().to_owned()),
        ),
    ]);
    properties.insert(
        "source_ref".to_owned(),
        event
            .source_ref
            .as_ref()
            .map_or(GraphValue::Null, |source_ref| {
                GraphValue::String(source_ref.clone())
            }),
    );
    properties
}

fn event_origin(event: &Event) -> Result<i64> {
    let event_id = event
        .event_id
        .ok_or_else(|| projector_error("event_id is required before projection"))?;
    i64::try_from(event_id)
        .map_err(|error| projector_error(format!("event_id out of range: {error}")).into())
}

fn event_timestamp(event: &Event) -> GraphValue {
    event
        .ts
        .map(|ts| GraphValue::String(ts.to_rfc3339()))
        .unwrap_or(GraphValue::Null)
}

fn relation_kind(kind: EventRelationKind) -> RelationKind {
    match kind {
        EventRelationKind::BasedOn => RelationKind::BasedOn,
        EventRelationKind::HasOption => RelationKind::HasOption,
        EventRelationKind::Chose => RelationKind::Chose,
        EventRelationKind::Assumes => RelationKind::PremisedOn,
        EventRelationKind::Supports => RelationKind::Supports,
        EventRelationKind::Refutes => RelationKind::Refutes,
        EventRelationKind::SameAs => RelationKind::SameAs,
        EventRelationKind::FollowsFrom => RelationKind::FollowsFrom,
    }
}

const fn project_link_relation_kind(kind: ProjectLinkKind) -> RelationKind {
    match kind {
        ProjectLinkKind::PartOf => RelationKind::PartOf,
        ProjectLinkKind::DependsOn => RelationKind::DependsOn,
    }
}

fn props_extend(
    origin: &GraphProperties,
    pairs: impl IntoIterator<Item = (&'static str, GraphValue)>,
) -> GraphProperties {
    let mut props = origin.clone();
    for (k, v) in pairs {
        props.insert(k.to_owned(), v); // ubs:ignore: &'static str key must become owned String for HashMap; not perf-sensitive (small fixed set of pairs per call)
    }
    props
}

/// The edges a decision rests on (hivemind-zdsh.15 §1): a prior decision, evidence, and an
/// assumption or bet (directly, or through the chosen option).
const fn is_grounding_relation(kind: RelationKind) -> bool {
    matches!(
        kind,
        RelationKind::FollowsFrom
            | RelationKind::BasedOn
            | RelationKind::PremisedOn
            | RelationKind::PremisedOnDirect
    )
}

/// Provenance of a grounding edge: who added it, when, and — for an edge named at capture —
/// the proposal event that caused it. Readers derive "at capture" vs "attributed later" from
/// `causation_event_id == the decision's proposal event` without a new ledger field.
/// `causation_event_id` is omitted (not written as null) when absent: Kuzu cannot store a
/// null string into its INT64 column.
fn grounding_edge_properties(
    origin_properties: &GraphProperties,
    actor_id: &str,
    added_at: GraphValue,
    causation_event_id: Option<i64>,
) -> GraphProperties {
    let mut properties = origin_properties.clone(); // ubs:ignore: one copy per grounding edge; the origin map is shared by every edge of the event
    properties.insert(
        "added_by".to_owned(),
        GraphValue::String(actor_id.to_owned()),
    );
    properties.insert("added_at".to_owned(), added_at);
    if let Some(causation_event_id) = causation_event_id {
        properties.insert(
            "causation_event_id".to_owned(),
            GraphValue::Int(causation_event_id),
        );
    }
    properties
}

fn project_decision_proposed(
    graph: &impl GraphView,
    actor_id: &str,
    payload: &DecisionProposedPayload,
    origin_properties: &GraphProperties,
    occurred_at: GraphValue,
) -> Result<()> {
    // No `project` on the payload (every event before hivemind-s15q.3, or a fresh one
    // written with no handle) projects to the recorder's personal project -- "no
    // migration, no rewrite" (approved record shape, item 8). `project_source` mirrors
    // that: absent only on pre-existing events, which read as `personal_fallback`.
    let (project, project_source) = match &payload.project {
        Some(handle) => (handle.clone(), payload.project_source),
        None => (
            personal_project_handle(actor_id),
            Some(
                payload
                    .project_source
                    .unwrap_or(ProjectSource::PersonalFallback),
            ),
        ),
    };
    // Edges named by the proposal itself are at capture by definition: their causation is
    // the proposal event, the same value the fan-out relation events carry.
    let proposal_event_origin = match origin_properties.get("event_origin") {
        Some(GraphValue::Int(event_origin)) => Some(*event_origin),
        _ => None,
    };
    let grounding_properties = grounding_edge_properties(
        origin_properties,
        actor_id,
        occurred_at.clone(), // ubs:ignore: the timestamp is also stored on the decision node below
        proposal_event_origin,
    );
    let decision_properties = props_extend(
        origin_properties,
        [
            ("title", GraphValue::String(payload.title.clone())),
            ("rationale", GraphValue::String(payload.rationale.clone())),
            (
                "topic_keys",
                GraphValue::StringList(payload.topic_keys.clone()),
            ),
            (
                "expressed_confidence",
                payload
                    .expressed_confidence
                    .as_deref()
                    .map_or(GraphValue::Null, |c| GraphValue::String(c.to_owned())),
            ),
            // Display-only: `event_origin` stays canonical for resolver/search ranking
            // (SEARCH_DESIGN.md). occurred_at gives DecisionBrief a human-readable "when".
            ("occurred_at", occurred_at),
            (
                "quote",
                payload
                    .quote
                    .as_deref()
                    .map_or(GraphValue::Null, |q| GraphValue::String(q.to_owned())),
            ),
            (
                "question",
                payload
                    .question
                    .as_deref()
                    .map_or(GraphValue::Null, |q| GraphValue::String(q.to_owned())),
            ),
            ("project", GraphValue::String(project)),
            (
                "project_source",
                project_source.map_or(GraphValue::Null, |source| {
                    GraphValue::String(source.as_str().to_owned())
                }),
            ),
        ],
    );
    graph.upsert_node(
        NodeKind::Decision,
        &payload.decision_id,
        &decision_properties,
    )?;
    graph.upsert_edge(
        RelationKind::ProposedBy,
        &payload.decision_id,
        actor_id,
        origin_properties,
    )?;

    // `None` when option_labels is shorter (events predating this field, or a caller that
    // never learned the label) — every reader of the "label" property (brief.rs, render.rs,
    // summarize.rs) already falls back to the option id when the property is absent. Shared
    // by the options loop below and the chosen-option upsert.
    let option_label = |option_id: &str| -> Option<String> {
        payload
            .option_ids
            .iter()
            .position(|id| id == option_id)
            .and_then(|index| payload.option_labels.get(index).cloned())
    };
    let option_description = |option_id: &str| -> Option<String> {
        payload
            .option_ids
            .iter()
            .position(|id| id == option_id)
            .and_then(|index| payload.option_descriptions.get(index).cloned())
    };
    // Historical events predate the option_labels field entirely and so never carry a real
    // label (hivemind-zdsh.10 migration). Those events' option ids are themselves a slug of
    // the label text (e.g. "option-other-rejected-options-<uuid>") from the id-generation
    // scheme that predated opaque ids; derive a readable label from that slug once, here at
    // projection time, rather than showing the raw id. `derive_legacy_option_label` returns
    // `None` (kept as Null, not re-indexed as a second copy of the id) when the id doesn't
    // decode into anything more readable than itself.
    let label_or_derived = |option_id: &str| -> GraphValue {
        match option_label(option_id) {
            Some(label) => GraphValue::String(label),
            None => {
                derive_legacy_option_label(option_id).map_or(GraphValue::Null, GraphValue::String)
            }
        }
    };

    for option_id in &payload.option_ids {
        // ubs:ignore: per-option props copy; each Option node needs a fresh map with a distinct "label" entry
        let mut option_properties = origin_properties.clone();
        option_properties.insert("label".to_owned(), label_or_derived(option_id)); // ubs:ignore: per-option label key; alloc differs per iteration — unavoidable with BTreeMap<String,…> properties map
        option_properties.insert(
            "description".to_owned(),
            option_description(option_id).map_or(GraphValue::Null, GraphValue::String),
        ); // ubs:ignore: per-option description key; alloc differs per iteration — unavoidable with BTreeMap<String,…> properties map
        graph.upsert_node(NodeKind::Option, option_id, &option_properties)?;
        graph.upsert_edge(
            RelationKind::HasOption,
            &payload.decision_id,
            option_id,
            origin_properties,
        )?;
    }

    if let Some(chosen_option_id) = &payload.chosen_option_id {
        // ubs:ignore: per-option props copy; the chosen option needs its own map with a distinct "label" entry, mirroring the options loop above
        let mut option_properties = origin_properties.clone();
        option_properties.insert("label".to_owned(), label_or_derived(chosen_option_id));
        option_properties.insert(
            "description".to_owned(),
            option_description(chosen_option_id).map_or(GraphValue::Null, GraphValue::String),
        );
        graph.upsert_node(NodeKind::Option, chosen_option_id, &option_properties)?;
        graph.upsert_edge(
            RelationKind::Chose,
            &payload.decision_id,
            chosen_option_id,
            origin_properties,
        )?;
    }

    let (premised_on_kind, premised_on_from) = if let Some(opt_id) = &payload.chosen_option_id {
        (RelationKind::PremisedOn, opt_id.as_str())
    } else {
        (RelationKind::PremisedOnDirect, payload.decision_id.as_str())
    };
    for hypothesis_id in &payload.hypothesis_ids {
        graph.upsert_edge(
            premised_on_kind,
            premised_on_from,
            hypothesis_id,
            &grounding_properties,
        )?;
    }

    for evidence_id in &payload.evidence_ids {
        graph.upsert_edge(
            RelationKind::BasedOn,
            &payload.decision_id,
            evidence_id,
            &grounding_properties,
        )?;
    }
    Ok(())
}

fn project_decision_requested(
    graph: &impl GraphView,
    event: &Event,
    payload: &DecisionRequestedPayload,
    origin_properties: &GraphProperties,
) -> Result<()> {
    let request_id = event.event_uuid.to_string();
    let mut request_properties = origin_properties.clone();
    request_properties.insert(
        "decision_id".to_owned(),
        optional_string_value(payload.decision_id.as_deref()),
    );
    request_properties.insert(
        "topic_keys".to_owned(),
        GraphValue::StringList(payload.topic_keys.clone()),
    );
    request_properties.insert(
        "reason".to_owned(),
        GraphValue::String(payload.reason.clone()),
    );
    request_properties.insert(
        "priority".to_owned(),
        GraphValue::String(payload.priority.as_str().to_owned()),
    );
    request_properties.insert(
        "required_owner_id".to_owned(),
        optional_string_value(payload.required_owner_id.as_deref()),
    );
    request_properties.insert(
        "authority_class".to_owned(),
        GraphValue::String(payload.authority_class.clone()),
    );
    request_properties.insert(
        "requested_by".to_owned(),
        GraphValue::String(payload.requested_by.clone()),
    );
    request_properties.insert(
        "client_request_id".to_owned(),
        GraphValue::String(payload.client_request_id.clone()),
    );
    graph.upsert_node(NodeKind::DecisionRequest, &request_id, &request_properties)?;

    upsert_actor(graph, &payload.requested_by, origin_properties)?;
    graph.upsert_edge(
        RelationKind::DecisionRequestedBy,
        &request_id,
        &payload.requested_by,
        origin_properties,
    )?;

    if let Some(required_owner_id) = &payload.required_owner_id {
        upsert_actor(graph, required_owner_id, origin_properties)?;
        graph.upsert_edge(
            RelationKind::DecisionRequestRequiredOwner,
            &request_id,
            required_owner_id,
            origin_properties,
        )?;
    }

    if let Some(decision_id) = &payload.decision_id {
        ensure_node_reference(graph, NodeKind::Decision, decision_id, origin_properties)?;
        graph.upsert_edge(
            RelationKind::DecisionRequestForDecision,
            &request_id,
            decision_id,
            origin_properties,
        )?;
    }
    Ok(())
}

fn project_evidence_recorded(
    graph: &impl GraphView,
    payload: &EvidenceRecordedPayload,
    origin_properties: &GraphProperties,
    recorded_at: GraphValue,
) -> Result<()> {
    // `source` on the node is the event's channel (cli, mcp, ...); the payload's `source` is
    // where the observation was made (URL, file@commit, measurement) — a different fact, so
    // it gets its own property.
    let props = props_extend(
        origin_properties,
        [
            ("content", GraphValue::String(payload.content.clone())),
            (
                "evidence_source",
                optional_string_value(payload.source.as_deref()),
            ),
            ("recorded_at", recorded_at),
        ],
    );
    graph.upsert_node(NodeKind::Evidence, &payload.evidence_id, &props)
}

fn project_hypothesis_recorded(
    graph: &impl GraphView,
    payload: &HypothesisRecordedPayload,
    origin_properties: &GraphProperties,
) -> Result<()> {
    let props = props_extend(
        origin_properties,
        [
            ("statement", GraphValue::String(payload.statement.clone())),
            (
                "kind",
                GraphValue::String(hypothesis_kind_str(payload.kind).to_owned()),
            ),
            (
                "check_by",
                payload
                    .check_by
                    .map_or(GraphValue::Null, |ts| GraphValue::String(ts.to_rfc3339())),
            ),
            (
                "would_change_if",
                optional_string_value(payload.would_change_if.as_deref()),
            ),
        ],
    );
    graph.upsert_node(NodeKind::Hypothesis, &payload.hypothesis_id, &props)
}

const fn hypothesis_kind_str(kind: events::HypothesisKind) -> &'static str {
    match kind {
        events::HypothesisKind::Assumption => "assumption",
        events::HypothesisKind::Bet => "bet",
    }
}

fn project_project_registered(
    graph: &impl GraphView,
    payload: &ProjectRegisteredPayload,
    origin_properties: &GraphProperties,
) -> Result<()> {
    let props = props_extend(
        origin_properties,
        [
            ("handle", GraphValue::String(payload.handle.clone())),
            (
                "display_name",
                optional_string_value(payload.display_name.as_deref()),
            ),
            ("purpose", optional_string_value(payload.purpose.as_deref())),
            ("anchors", GraphValue::StringList(Vec::new())),
        ],
    );
    graph.upsert_node(NodeKind::Project, &payload.handle, &props)
}

/// Encodes one anchor as a single self-describing string (`"{kind}:{value}"`) so it can
/// live in the `anchors` StringList property on the Project node instead of a separate
/// edge table — matching the approved record shape's "props ... anchors[]".
fn encode_project_anchor(anchor_kind: ProjectAnchorKind, value: &str) -> String {
    format!("{}:{value}", anchor_kind.as_str())
}

/// `upsert_node` merges properties by key (last write wins per key, never appends into an
/// existing list — see `GraphView::upsert_node`), so accumulating the `anchors` list across
/// multiple `project.anchored`/`project.unanchored` events needs a read before the write.
/// Reuses the existing "all nodes of a kind" query shape (see `map::load_decisions`) rather
/// than adding a new query pattern to every `GraphView` backend: memory/postgres already
/// return every stored property for that shape regardless of the requested columns, and Kuzu
/// (real Cypher) returns exactly the two columns named here.
fn current_project_anchors(graph: &impl GraphView, handle: &str) -> Result<Vec<String>> {
    let rows = graph.query(
        "MATCH (node:`Project`) RETURN node.id AS id, node.anchors AS anchors ORDER BY node.id;",
        &GraphParams::new(),
    )?;
    for row in &rows {
        if matches!(row.get("id"), Some(GraphValue::String(id)) if id == handle) {
            return Ok(match row.get("anchors") {
                Some(GraphValue::StringList(values)) => values.clone(), // ubs:ignore: clone necessary — GraphRow holds borrowed ref, function returns owned Vec<String>
                _ => Vec::new(),
            });
        }
    }
    Ok(Vec::new())
}

fn project_project_anchored(
    graph: &impl GraphView,
    payload: &ProjectAnchorPayload,
    origin_properties: &GraphProperties,
) -> Result<()> {
    let entry = encode_project_anchor(payload.anchor_kind, &payload.value);
    let mut anchors = current_project_anchors(graph, &payload.handle)?;
    if !anchors.contains(&entry) {
        anchors.push(entry);
    }
    let props = GraphProperties::from([("anchors".to_owned(), GraphValue::StringList(anchors))]);
    annotate_node(
        graph,
        NodeKind::Project,
        &payload.handle,
        origin_properties,
        &props,
    )
}

fn project_project_unanchored(
    graph: &impl GraphView,
    payload: &ProjectAnchorPayload,
    origin_properties: &GraphProperties,
) -> Result<()> {
    let entry = encode_project_anchor(payload.anchor_kind, &payload.value);
    let mut anchors = current_project_anchors(graph, &payload.handle)?;
    anchors.retain(|existing| existing != &entry);
    let props = GraphProperties::from([("anchors".to_owned(), GraphValue::StringList(anchors))]);
    annotate_node(
        graph,
        NodeKind::Project,
        &payload.handle,
        origin_properties,
        &props,
    )
}

fn project_blocker_reported(
    graph: &impl GraphView,
    event: &Event,
    event_origin: i64,
    payload: &BlockerReportedPayload,
    origin_properties: &GraphProperties,
) -> Result<()> {
    let mut blocker_properties = origin_properties.clone();
    blocker_properties.insert(
        "blocked_actor_id".to_owned(),
        GraphValue::String(payload.blocked_actor_id.clone()),
    );
    blocker_properties.insert(
        "decision_id".to_owned(),
        optional_string_value(payload.decision_id.as_deref()),
    );
    blocker_properties.insert(
        "topic_keys".to_owned(),
        GraphValue::StringList(payload.topic_keys.clone()),
    );
    blocker_properties.insert(
        "blocked_ref".to_owned(),
        GraphValue::String(payload.blocked_ref.clone()),
    );
    blocker_properties.insert(
        "blocked_ref_type".to_owned(),
        GraphValue::String(payload.blocked_ref_type.clone()),
    );
    blocker_properties.insert(
        "reason".to_owned(),
        GraphValue::String(payload.reason.clone()),
    );
    blocker_properties.insert(
        "priority".to_owned(),
        GraphValue::String(payload.priority.as_str().to_owned()),
    );
    blocker_properties.insert(
        "last_progress_at".to_owned(),
        payload
            .last_progress_at
            .map(|timestamp| GraphValue::String(timestamp.to_rfc3339()))
            .unwrap_or(GraphValue::Null),
    );
    blocker_properties.insert(
        "required_owner_id".to_owned(),
        optional_string_value(payload.required_owner_id.as_deref()),
    );
    blocker_properties.insert("reported_at".to_owned(), event_timestamp(event));
    blocker_properties.insert(
        "reported_event_origin".to_owned(),
        GraphValue::Int(event_origin),
    );
    graph.upsert_node(NodeKind::Blocker, &payload.blocker_id, &blocker_properties)?;

    upsert_actor(graph, &payload.blocked_actor_id, origin_properties)?;
    graph.upsert_edge(
        RelationKind::BlockedActor,
        &payload.blocker_id,
        &payload.blocked_actor_id,
        origin_properties,
    )?;

    if let Some(required_owner_id) = &payload.required_owner_id {
        upsert_actor(graph, required_owner_id, origin_properties)?;
        graph.upsert_edge(
            RelationKind::BlockerRequiredOwner,
            &payload.blocker_id,
            required_owner_id,
            origin_properties,
        )?;
    }

    if let Some(decision_id) = &payload.decision_id {
        ensure_node_reference(graph, NodeKind::Decision, decision_id, origin_properties)?;
        graph.upsert_edge(
            RelationKind::BlockerForDecision,
            &payload.blocker_id,
            decision_id,
            origin_properties,
        )?;
    }
    Ok(())
}

fn project_blocker_resolved(
    graph: &impl GraphView,
    event: &Event,
    event_origin: i64,
    payload: &BlockerResolvedPayload,
    origin_properties: &GraphProperties,
) -> Result<()> {
    let blocker_properties = GraphProperties::from([
        ("resolved_at".to_owned(), event_timestamp(event)),
        (
            "resolution_event_id".to_owned(),
            payload
                .resolution_event_id
                .and_then(|id| i64::try_from(id).ok())
                .map_or(GraphValue::Null, GraphValue::Int),
        ),
        (
            "resolution_reason".to_owned(),
            payload
                .resolution_reason
                .clone()
                .map_or(GraphValue::Null, GraphValue::String),
        ),
        (
            "resolved_event_origin".to_owned(),
            GraphValue::Int(event_origin),
        ),
    ]);
    annotate_node(
        graph,
        NodeKind::Blocker,
        &payload.blocker_id,
        origin_properties,
        &blocker_properties,
    )
}

fn project_notification_sent(
    graph: &impl GraphView,
    event: &Event,
    payload: &NotificationSentPayload,
    origin_properties: &GraphProperties,
) -> Result<()> {
    let notification_id = event.event_uuid.to_string();
    let mut notification_properties = origin_properties.clone();
    notification_properties.insert(
        "blocker_id".to_owned(),
        GraphValue::String(payload.blocker_id.clone()),
    );
    notification_properties.insert(
        "recipient_actor_id".to_owned(),
        GraphValue::String(payload.recipient_actor_id.clone()),
    );
    notification_properties.insert(
        "channel".to_owned(),
        GraphValue::String(payload.channel.clone()),
    );
    notification_properties.insert(
        "threshold_rule".to_owned(),
        GraphValue::String(payload.threshold_rule.clone()),
    );
    notification_properties.insert(
        "source_event_ids".to_owned(),
        GraphValue::StringList(
            payload
                .source_event_ids
                .iter()
                .map(|event_id| event_id.to_string())
                .collect(),
        ),
    );
    notification_properties.insert(
        "dedupe_key".to_owned(),
        GraphValue::String(payload.dedupe_key.clone()),
    );
    notification_properties.insert(
        "sent_at".to_owned(),
        GraphValue::String(payload.sent_at.to_rfc3339()),
    );
    graph.upsert_node(
        NodeKind::Notification,
        &notification_id,
        &notification_properties,
    )?;

    ensure_node_reference(
        graph,
        NodeKind::Blocker,
        &payload.blocker_id,
        origin_properties,
    )?;
    graph.upsert_edge(
        RelationKind::NotificationForBlocker,
        &notification_id,
        &payload.blocker_id,
        origin_properties,
    )?;

    upsert_actor(graph, &payload.recipient_actor_id, origin_properties)?;
    graph.upsert_edge(
        RelationKind::NotificationRecipient,
        &notification_id,
        &payload.recipient_actor_id,
        origin_properties,
    )?;
    Ok(())
}

fn project_notification_acknowledged(
    graph: &impl GraphView,
    payload: &NotificationAcknowledgedPayload,
    origin_properties: &GraphProperties,
) -> Result<()> {
    let props = GraphProperties::from([
        (
            "ack_at".to_owned(),
            GraphValue::String(payload.ack_at.to_rfc3339()),
        ),
        (
            "snooze_until".to_owned(),
            payload
                .snooze_until
                .map(|value| GraphValue::String(value.to_rfc3339()))
                .unwrap_or(GraphValue::Null),
        ),
    ]);
    annotate_node(
        graph,
        NodeKind::Notification,
        &payload.notification_id,
        origin_properties,
        &props,
    )
}

fn project_ingest_batch_classified(
    graph: &impl GraphView,
    event_origin: i64,
    payload: &IngestBatchClassifiedPayload,
    origin_properties: &GraphProperties,
) -> Result<()> {
    let node_ids: Vec<String> = (0..payload.captures.len())
        .map(|idx| format!("capture:{event_origin}:{idx}"))
        .collect();
    let id_captures: Vec<(&str, &CaptureItem)> = node_ids
        .iter()
        .map(String::as_str)
        .zip(payload.captures.iter())
        .collect();
    let (resolved, _stats) = resolve_batch_local_references(&id_captures);
    for (node_id, capture) in &resolved {
        project_capture(graph, capture, node_id, origin_properties)?;
    }
    Ok(())
}

fn project_decision_scored(
    graph: &impl GraphView,
    payload: &DecisionScoredPayload,
    origin_properties: &GraphProperties,
) -> Result<()> {
    // Annotate the capture node with per-dimension Quality scores and
    // Importance factors. Upsert merges onto the existing node without
    // overwriting any decision fields, including the offset that created it.
    let dims = &payload.quality_dims;
    let imp = &payload.importance;
    let props = props_extend(
        &GraphProperties::new(),
        [
            ("score_framing", GraphValue::Float(dims.framing.score)),
            (
                "score_alternatives",
                GraphValue::Float(dims.alternatives.score),
            ),
            (
                "score_information",
                GraphValue::Float(dims.information.score),
            ),
            ("score_reasoning", GraphValue::Float(dims.reasoning.score)),
            (
                "score_values_tradeoffs",
                GraphValue::Float(dims.values_tradeoffs.score),
            ),
            (
                "score_bias_exposure",
                GraphValue::Float(dims.bias_exposure.score),
            ),
            (
                "score_calibration",
                GraphValue::Float(dims.calibration.score),
            ),
            (
                "score_weight_version",
                GraphValue::String(payload.weight_version.clone()),
            ),
            ("importance_stakes", GraphValue::Float(imp.stakes)),
            (
                "importance_irreversibility",
                GraphValue::Float(imp.irreversibility),
            ),
            (
                "importance_actionability",
                GraphValue::Float(imp.actionability),
            ),
        ],
    );
    annotate_node(
        graph,
        NodeKind::Decision,
        &payload.capture_node_id,
        origin_properties,
        &props,
    )
}

/// `decision.moved` upserts ONLY `project` and `project_source = moved` on the existing Decision
/// node (approved record shape, item 3) — never the event's origin properties, the way the
/// delegation marker above upserts only its marker. A move changes where a decision is found,
/// not who captured it, so the node keeps the proposal's own `event_origin` / `source` /
/// `source_ref` / `tenant_id`: attribution stays with the capture, and anything ordering or
/// filtering by `event_origin` does not see an old moved decision as new. `upsert_node` merges
/// by key, so title, rationale, options and the rest are untouched too. The move fact itself
/// lives in the ledger and is surfaced in history (`queries::history`); this only keeps the
/// node's current project in sync with it.
fn project_decision_moved(graph: &impl GraphView, payload: &DecisionMovedPayload) -> Result<()> {
    graph.upsert_node(
        NodeKind::Decision,
        &payload.decision_id,
        &GraphProperties::from([
            ("project".to_owned(), GraphValue::String(payload.to.clone())),
            (
                "project_source".to_owned(),
                GraphValue::String(ProjectSource::Moved.as_str().to_owned()),
            ),
        ]),
    )
}

fn project_capture(
    graph: &impl GraphView,
    capture: &CaptureItem,
    node_id: &str,
    origin_properties: &GraphProperties,
) -> Result<()> {
    match capture.kind.as_str() {
        "decision" => project_capture_decision(graph, capture, node_id, origin_properties),
        "evidence" => project_capture_evidence(graph, capture, node_id, origin_properties),
        "hypothesis" => project_capture_hypothesis(graph, capture, node_id, origin_properties),
        "blocker" => project_capture_blocker(graph, capture, node_id, origin_properties),
        "decision-request" => {
            project_capture_decision_request(graph, capture, node_id, origin_properties)
        }
        "notification" => project_capture_notification(graph, capture, node_id, origin_properties),
        kind => {
            tracing::debug!(
                target: "hivemind::projector",
                "skipping capture with unrecognised kind: {kind}"
            );
            Ok(())
        }
    }
}

fn project_capture_decision(
    graph: &impl GraphView,
    capture: &CaptureItem,
    node_id: &str,
    origin_properties: &GraphProperties,
) -> Result<()> {
    let mut props = origin_properties.clone();
    props.insert(
        "title".to_owned(),
        GraphValue::String(capture.title.clone()),
    );
    props.insert(
        "rationale".to_owned(),
        GraphValue::String(capture.rationale.clone()),
    );
    props.insert(
        "topic_keys".to_owned(),
        GraphValue::StringList(capture.topic_keys.clone()),
    );
    props.insert(
        "expressed_confidence".to_owned(),
        capture
            .expressed_confidence
            .as_deref()
            .map_or(GraphValue::Null, |c| GraphValue::String(c.to_owned())),
    );
    graph.upsert_node(NodeKind::Decision, node_id, &props)?;

    if let Some(actor_id) = &capture.actor_id {
        upsert_actor(graph, actor_id, origin_properties)?;
        graph.upsert_edge(
            RelationKind::ProposedBy,
            node_id,
            actor_id,
            origin_properties,
        )?;
    }
    for accepted_by in &capture.accepted_by {
        upsert_actor(graph, accepted_by, origin_properties)?;
        graph.upsert_edge(
            RelationKind::AcceptedBy,
            node_id,
            accepted_by,
            origin_properties,
        )?;
    }
    for rejected_by in &capture.rejected_by {
        upsert_actor(graph, rejected_by, origin_properties)?;
        graph.upsert_edge(
            RelationKind::RejectedBy,
            node_id,
            rejected_by,
            origin_properties,
        )?;
    }
    if let Some(supersedes_id) = &capture.supersedes_id {
        ensure_node_reference(graph, NodeKind::Decision, supersedes_id, origin_properties)?;
        graph.upsert_edge(
            RelationKind::Supersedes,
            node_id,
            supersedes_id,
            origin_properties,
        )?;
    }
    let (premised_on_kind, premised_on_from) = if let Some(chosen) = &capture.chosen_option {
        (RelationKind::PremisedOn, option_node_id(node_id, chosen))
    } else {
        (RelationKind::PremisedOnDirect, node_id.to_owned())
    };
    for hypothesis_id in &capture.premised_on_ids {
        ensure_node_reference(
            graph,
            NodeKind::Hypothesis,
            hypothesis_id,
            origin_properties,
        )?;
        graph.upsert_edge(
            premised_on_kind,
            &premised_on_from,
            hypothesis_id,
            origin_properties,
        )?;
    }
    for evidence_id in &capture.evidence_ids {
        ensure_node_reference(graph, NodeKind::Evidence, evidence_id, origin_properties)?;
        graph.upsert_edge(
            RelationKind::BasedOn,
            node_id,
            evidence_id,
            origin_properties,
        )?;
    }
    for participant_id in &capture.participants {
        upsert_actor(graph, participant_id, origin_properties)?;
        graph.upsert_edge(
            RelationKind::ParticipatedBy,
            node_id,
            participant_id,
            origin_properties,
        )?;
    }
    if let Some(initiator_id) = &capture.session_initiator {
        upsert_actor(graph, initiator_id, origin_properties)?;
        graph.upsert_edge(
            RelationKind::InitiatedBy,
            node_id,
            initiator_id,
            origin_properties,
        )?;
    }
    // Options: project Option nodes + HAS_OPTION/CHOSE edges.
    if let Some(options) = &capture.options {
        for option_label in options {
            let opt_id = option_node_id(node_id, option_label);
            let mut opt_props = origin_properties.clone(); // ubs:ignore: per-option props copy; each Option node needs a fresh map with a distinct "label" entry
            opt_props.insert("label".to_owned(), GraphValue::String(option_label.clone())); // ubs:ignore: per-option label; key alloc + value clone differ per iteration — unavoidable with BTreeMap<String,…>
            graph.upsert_node(NodeKind::Option, &opt_id, &opt_props)?;
            graph.upsert_edge(RelationKind::HasOption, node_id, &opt_id, origin_properties)?;
        }
    }
    if let Some(chosen) = &capture.chosen_option {
        let opt_id = option_node_id(node_id, chosen);
        graph.upsert_edge(RelationKind::Chose, node_id, &opt_id, origin_properties)?;
    }
    Ok(())
}

fn project_capture_evidence(
    graph: &impl GraphView,
    capture: &CaptureItem,
    node_id: &str,
    origin_properties: &GraphProperties,
) -> Result<()> {
    let mut props = origin_properties.clone();
    props.insert(
        "content".to_owned(),
        GraphValue::String(capture.title.clone()),
    );
    props.insert(
        "topic_keys".to_owned(),
        GraphValue::StringList(capture.topic_keys.clone()),
    );
    graph.upsert_node(NodeKind::Evidence, node_id, &props)?;

    for hypothesis_id in &capture.supports_ids {
        ensure_node_reference(
            graph,
            NodeKind::Hypothesis,
            hypothesis_id,
            origin_properties,
        )?;
        graph.upsert_edge(
            RelationKind::Supports,
            node_id,
            hypothesis_id,
            origin_properties,
        )?;
    }
    for hypothesis_id in &capture.refutes_ids {
        ensure_node_reference(
            graph,
            NodeKind::Hypothesis,
            hypothesis_id,
            origin_properties,
        )?;
        graph.upsert_edge(
            RelationKind::Refutes,
            node_id,
            hypothesis_id,
            origin_properties,
        )?;
    }
    Ok(())
}

fn project_capture_hypothesis(
    graph: &impl GraphView,
    capture: &CaptureItem,
    node_id: &str,
    origin_properties: &GraphProperties,
) -> Result<()> {
    let mut props = origin_properties.clone();
    props.insert(
        "statement".to_owned(),
        GraphValue::String(capture.title.clone()),
    );
    graph.upsert_node(NodeKind::Hypothesis, node_id, &props)
}

fn project_capture_blocker(
    graph: &impl GraphView,
    capture: &CaptureItem,
    node_id: &str,
    origin_properties: &GraphProperties,
) -> Result<()> {
    let mut props = origin_properties.clone();
    props.insert(
        "reason".to_owned(),
        GraphValue::String(capture.title.clone()),
    );
    props.insert(
        "topic_keys".to_owned(),
        GraphValue::StringList(capture.topic_keys.clone()),
    );
    graph.upsert_node(NodeKind::Blocker, node_id, &props)?;

    if let Some(blocked_actor_id) = &capture.blocked_actor_id {
        upsert_actor(graph, blocked_actor_id, origin_properties)?;
        graph.upsert_edge(
            RelationKind::BlockedActor,
            node_id,
            blocked_actor_id,
            origin_properties,
        )?;
    }
    if let Some(decision_id) = &capture.decision_id {
        ensure_node_reference(graph, NodeKind::Decision, decision_id, origin_properties)?;
        graph.upsert_edge(
            RelationKind::BlockerForDecision,
            node_id,
            decision_id,
            origin_properties,
        )?;
    }
    Ok(())
}

fn project_capture_decision_request(
    graph: &impl GraphView,
    capture: &CaptureItem,
    node_id: &str,
    origin_properties: &GraphProperties,
) -> Result<()> {
    let mut props = origin_properties.clone();
    props.insert(
        "reason".to_owned(),
        GraphValue::String(capture.title.clone()),
    );
    props.insert(
        "topic_keys".to_owned(),
        GraphValue::StringList(capture.topic_keys.clone()),
    );
    graph.upsert_node(NodeKind::DecisionRequest, node_id, &props)?;

    // A DecisionRequest with a recorded accept/reject position is a contested ask
    // ("X proposed Y; Z rejected") rather than a plain open request ("owner raised
    // the request") — deterministic Layer-2 rule over already-extracted fields, same
    // pattern as PremisedOn vs PremisedOnDirect above. Picking RequestProposedBy over
    // DecisionRequestedBy here (rather than always emitting DecisionRequestedBy) keeps
    // D1/D3 (plain asks, no accepted_by/rejected_by) on their existing edge kind.
    let contested = !capture.accepted_by.is_empty() || !capture.rejected_by.is_empty();
    if let Some(actor_id) = &capture.actor_id {
        upsert_actor(graph, actor_id, origin_properties)?;
        let kind = if contested {
            RelationKind::RequestProposedBy
        } else {
            RelationKind::DecisionRequestedBy
        };
        graph.upsert_edge(kind, node_id, actor_id, origin_properties)?;
    }
    for accepted_by in &capture.accepted_by {
        upsert_actor(graph, accepted_by, origin_properties)?;
        graph.upsert_edge(
            RelationKind::RequestAcceptedBy,
            node_id,
            accepted_by,
            origin_properties,
        )?;
    }
    for rejected_by in &capture.rejected_by {
        upsert_actor(graph, rejected_by, origin_properties)?;
        graph.upsert_edge(
            RelationKind::RequestRejectedBy,
            node_id,
            rejected_by,
            origin_properties,
        )?;
    }
    Ok(())
}

fn project_capture_notification(
    graph: &impl GraphView,
    capture: &CaptureItem,
    node_id: &str,
    origin_properties: &GraphProperties,
) -> Result<()> {
    let mut props = origin_properties.clone();
    props.insert(
        "channel".to_owned(),
        GraphValue::String(capture.title.clone()),
    );
    graph.upsert_node(NodeKind::Notification, node_id, &props)
}

fn projector_error(error: impl std::fmt::Display) -> ProjectorError {
    ProjectorError::Projection(error.to_string())
}

#[cfg(test)]
mod tests;
