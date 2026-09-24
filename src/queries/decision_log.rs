//! Pure Markdown projection of the decision log, grouped per project: an `INDEX.md` with one
//! section per project, and for each project a `projects/<handle>/INDEX.md` plus one file per
//! decision under `projects/<handle>/decisions/`. Personal projects (derived from the actor,
//! never registered) live under `projects/personal/<actor>/`. Every decision belongs to exactly
//! one project, so it is exported exactly once.
//!
//! Layer 2: deterministic graph reads composed with a single ledger-offset lookup for
//! provenance. No LLM, no ranking, no wall clock — every timestamp rendered here comes
//! from the ledger's own `occurred_at` / event timestamps (AGENTS.md §1.6, §6).
//!
//! Per-decision facts are read directly (via `get_decision_brief` / `get_decision` /
//! one-hop `SUPERSEDES` neighbor pairs), not via `get_compact_view`: that function resolves
//! to the *terminal* decision of a supersession chain, which is correct for "what's true
//! now" follow-up queries but wrong for a historical log entry about a specific
//! (possibly superseded) decision. `get_supersession_chain`'s multi-hop walk is also
//! avoided here because it errors on a branched chain (more than one direct superseder);
//! an export must still render every decision, so branches are surfaced as a list rather
//! than treated as fatal.
//!
//! Graph reads reuse the same generic, shared helpers (`node_rows`, `neighbor_pairs`) that
//! every other query module uses, rather than hand-rolled Cypher: bespoke query strings
//! have no fixture support in the in-memory `GraphView` test double
//! (`src/projector/memory.rs` pattern-matches known query shapes), and reuse also means one
//! fewer round trip per decision for evidence/decision property lookups.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};

use crate::commands::{
    normalize_topic_key, personal_project_handle, PERSONAL_PROJECT_HANDLE_PREFIX,
};
use crate::events::EventId;
use crate::ledger::EventLedger;
use crate::projector::{GraphRow, GraphView, NodeKind, RelationKind};
use crate::Result;

use super::active_blockers::{
    get_active_decision_blockers, ActiveDecisionBlockersRequest, DecisionBlockerFilters,
    DecisionBlockerView,
};
use super::brief::{get_decision_brief, OptionLabel};
use super::decision::{get_decision, get_hypothesis_statement};
use super::grounding::{GroundingItem, GroundingKind, GroundingState};
use super::outcome::OutcomeReason;
use super::projects::{is_known_project, registered_project_names};
use super::shared::{
    neighbor_pairs, node_rows, optional_int, optional_string, query_error, Direction,
    MAX_QUERY_RESULTS,
};
use super::status::{DecisionStatus, HypothesisStatus};

const MAX_SLUG_LEN: usize = 60;
const ID8_LEN: usize = 8;
const PROJECTS_DIR: &str = "projects";
const PERSONAL_DIR: &str = "personal";
const DECISIONS_DIR: &str = "decisions";
const INDEX_FILE: &str = "INDEX.md";

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Filters are ANDed. Empty `topics` / `statuses` mean "no restriction on that axis".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DecisionLogRequest {
    pub since: Option<DateTime<Utc>>,
    pub topics: Vec<String>,
    pub statuses: Vec<DecisionStatus>,
    /// Only this project's decisions: a registered handle or a `personal:` address. A handle
    /// that resolves to neither yields `DecisionLogOutcome::ProjectNotFound`, never an empty
    /// export that reads like "that project has no decisions".
    pub project: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DecisionLogExport {
    pub ledger_offset: EventId,
    /// Relative path -> file content. Always includes the root `"INDEX.md"`; for each project
    /// in the export (see [`export_decision_log`]) a `projects/<dir>/INDEX.md` plus one entry
    /// per exported decision under `projects/<dir>/decisions/`.
    pub files: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DecisionLogOutcome {
    Exported(DecisionLogExport),
    /// A miss is data, not an error (Alex's rule, 2026-09-20): `project` is neither a
    /// registered handle nor a personal address. Nothing was exported.
    ProjectNotFound {
        project: String,
    },
}

/// Compose per-decision facts into a full Markdown export, grouped per project. Pure
/// projection — no filesystem writes; the caller decides where these go.
///
/// The projects in the export are every registered project plus every project a matching
/// decision belongs to (so a registered project with no decisions still gets its — empty —
/// record, and a personal project appears as soon as it holds a decision); with
/// `req.project` set, exactly that one.
pub fn export_decision_log(
    graph: &impl GraphView,
    ledger: &impl EventLedger,
    req: &DecisionLogRequest,
) -> Result<DecisionLogOutcome> {
    let registered = registered_project_names(ledger)?;
    let requested = match req.project.as_deref().map(str::trim) {
        Some("") => return Err(query_error("project must not be empty").into()),
        Some(handle) => Some(handle),
        None => None,
    };
    if let Some(handle) = requested {
        if !is_known_project(&registered, handle) {
            return Ok(DecisionLogOutcome::ProjectNotFound {
                project: handle.to_owned(),
            });
        }
    }

    let ledger_offset = ledger.latest_offset()?;
    let last_event_ts = last_event_timestamp(ledger, ledger_offset)?;

    let decision_rows = node_rows(graph, NodeKind::Decision)?;
    let evidence_rows = node_rows(graph, NodeKind::Evidence)?;
    let titles: BTreeMap<String, String> = decision_rows
        .iter()
        .map(|(id, row)| {
            (
                id.clone(), // ubs:ignore: clone necessary — building owned titles map while decision_rows stays borrowed for the loop below
                optional_string(row, "title").unwrap_or_default(),
            )
        })
        .collect();

    let mut entries = Vec::new();
    for id in decision_rows.keys() {
        let entry = build_entry(graph, id, &decision_rows, &evidence_rows)?;
        if matches_filters(&entry, req, requested) {
            entries.push(entry);
        }
    }
    entries.sort_by(|a, b| {
        (a.occurred_at, a.event_origin, &a.id).cmp(&(b.occurred_at, b.event_origin, &b.id))
    });

    let handles: BTreeSet<&str> = match requested {
        Some(handle) => BTreeSet::from([handle]),
        None => registered
            .keys()
            .map(String::as_str)
            .chain(entries.iter().map(|entry| entry.project.as_str()))
            .collect(),
    };
    let projects = assign_projects(&handles, &registered);
    let mut by_project: BTreeMap<&str, Vec<&DecisionEntry>> = BTreeMap::new();
    for entry in &entries {
        by_project
            .entry(entry.project.as_str())
            .or_default()
            .push(entry);
    }

    let filenames = assign_filenames(&entries);
    let mut paths = BTreeMap::new();
    for project in &projects {
        for entry in members_of(&by_project, project) {
            paths.insert(
                entry.id.clone(), // ubs:ignore: clone necessary — owned key for the paths map, entry stays borrowed for the render loop below
                decision_path(project, exported_filename(&filenames, &entry.id)),
            );
        }
    }

    let mut files = BTreeMap::new();
    for project in &projects {
        let members = members_of(&by_project, project);
        let decisions_dir = project.decisions_dir();
        for entry in members {
            files.insert(
                decision_path(project, exported_filename(&filenames, &entry.id)),
                render_decision_file(entry, &paths, &titles, &decisions_dir),
            );
        }
        files.insert(
            project.index_path(),
            render_project_index(
                project,
                members,
                req,
                ledger_offset,
                last_event_ts,
                &paths,
                &titles,
            ),
        );
    }
    files.insert(
        INDEX_FILE.to_owned(),
        render_root_index(
            &projects,
            &by_project,
            req,
            ledger_offset,
            last_event_ts,
            &paths,
            &titles,
        ),
    );

    Ok(DecisionLogOutcome::Exported(DecisionLogExport {
        ledger_offset,
        files,
    }))
}

// ---------------------------------------------------------------------------
// Per-decision data gathering
// ---------------------------------------------------------------------------

struct DecisionEntry {
    id: String,
    /// The project this decision belongs to: a registered handle or a `personal:` address.
    project: String,
    title: String,
    rationale: String,
    quote: Option<String>,
    question: Option<String>,
    status: DecisionStatus,
    occurred_at: Option<DateTime<Utc>>,
    event_origin: Option<i64>,
    topic_keys: Vec<String>,
    proposer_id: Option<String>,
    source: String,
    source_ref: Option<String>,
    chosen_option: Option<OptionLabel>,
    rejected_options: Vec<OptionLabel>,
    hypotheses: Vec<HypothesisEntry>,
    evidence: Vec<EvidenceEntry>,
    /// What the decision rests on, each with its state and provenance.
    rests_on: Vec<GroundingItem>,
    grounding_state: GroundingState,
    expressed_confidence: Option<String>,
    /// How many other decisions follow from this one.
    dependents_count: usize,
    held_up: bool,
    outcome_reasons: Vec<OutcomeReason>,
    accepted_by: Vec<String>,
    rejected_by: Vec<String>,
    /// Immediate predecessor (this decision's own `SUPERSEDES` target), if any.
    supersedes: Option<String>,
    /// Every decision that directly supersedes this one. Normally 0 or 1; more than one
    /// means a branched chain (AGENTS.md §3: `contested`/staleness must stay visible, not
    /// collapsed to a single silently-chosen value).
    all_superseded_by: Vec<String>,
    active_blockers: Vec<DecisionBlockerView>,
}

struct HypothesisEntry {
    id: String,
    statement: String,
    status: HypothesisStatus,
}

struct EvidenceEntry {
    content: String,
    source_ref: Option<String>,
}

fn build_entry(
    graph: &impl GraphView,
    id: &str,
    decision_rows: &BTreeMap<String, GraphRow>,
    evidence_rows: &BTreeMap<String, GraphRow>,
) -> Result<DecisionEntry> {
    let brief = get_decision_brief(graph, id)?
        .data
        .ok_or_else(|| query_error(format!("decision {id} disappeared mid-export")))?;
    let decision = get_decision(graph, id)?
        .data
        .ok_or_else(|| query_error(format!("decision {id} disappeared mid-export")))?;
    let row = decision_rows.get(id);
    let event_origin = row.and_then(|row| optional_int(row, "event_origin"));

    let mut hypotheses = Vec::with_capacity(decision.hypotheses.len());
    for hyp in &decision.hypotheses {
        let statement = get_hypothesis_statement(graph, &hyp.id)?.unwrap_or_default();
        hypotheses.push(HypothesisEntry {
            id: hyp.id.clone(), // ubs:ignore: clone necessary — building owned HypothesisEntry from borrowed hyp
            statement,
            status: hyp.status,
        });
    }

    let mut evidence = Vec::with_capacity(decision.evidence_ids.len());
    for evidence_id in &decision.evidence_ids {
        let (content, source_ref) = match evidence_rows.get(evidence_id) {
            Some(row) => (
                optional_string(row, "content").unwrap_or_default(),
                optional_string(row, "source_ref"),
            ),
            None => (String::new(), None),
        };
        evidence.push(EvidenceEntry {
            content,
            source_ref,
        });
    }

    let accepted_by = actor_ids(graph, id, RelationKind::AcceptedBy)?;
    let rejected_by = actor_ids(graph, id, RelationKind::RejectedBy)?;

    // One-hop SUPERSEDES neighbors via the same generic, MemoryGraph-backed helper every
    // other query module uses — not `get_supersession_chain`'s multi-hop walk, which errors
    // on a branch (see module docs).
    let supersedes = neighbor_pairs(
        graph,
        NodeKind::Decision,
        id,
        RelationKind::Supersedes,
        NodeKind::Decision,
        Direction::Outgoing,
    )?
    .into_iter()
    .map(|(older_id, _)| older_id)
    .next();
    let all_superseded_by: Vec<String> = neighbor_pairs(
        graph,
        NodeKind::Decision,
        id,
        RelationKind::Supersedes,
        NodeKind::Decision,
        Direction::Incoming,
    )?
    .into_iter()
    .map(|(newer_id, _)| newer_id)
    .collect();

    let active_blockers = get_active_decision_blockers(
        graph,
        &ActiveDecisionBlockersRequest {
            filters: DecisionBlockerFilters {
                decision_ids: vec![id.to_owned()],
                ..DecisionBlockerFilters::default()
            },
            limit: MAX_QUERY_RESULTS,
            cursor: None,
        },
    )?
    .data
    .items;

    // A graph projected before decisions carried a project has no property: it reads as the
    // recorder's personal project, the same rule the projector applies to old events.
    let project = match row.and_then(|row| optional_string(row, "project")) {
        Some(project) => project,
        None => {
            let proposer = brief.decided_by.proposer_id.as_deref().ok_or_else(|| {
                query_error(format!(
                    "decision {id} has neither a project nor a proposer to derive one from"
                ))
            })?;
            personal_project_handle(proposer)
        }
    };

    Ok(DecisionEntry {
        id: decision.id,
        project,
        title: decision.title,
        rationale: decision.rationale,
        quote: decision.quote,
        question: decision.question,
        status: decision.status,
        occurred_at: brief.occurred_at,
        event_origin,
        topic_keys: decision.topic_keys,
        proposer_id: brief.decided_by.proposer_id,
        source: brief.decided_by.source,
        source_ref: brief.decided_by.source_ref,
        chosen_option: brief.chosen_option,
        rejected_options: brief.rejected_options,
        hypotheses,
        evidence,
        rests_on: brief.rests_on,
        grounding_state: brief.grounding_state,
        expressed_confidence: brief.expressed_confidence,
        dependents_count: brief.dependents_count,
        held_up: brief.still_holds.held_up,
        outcome_reasons: brief.still_holds.reasons,
        accepted_by,
        rejected_by,
        supersedes,
        all_superseded_by,
        active_blockers,
    })
}

fn matches_filters(entry: &DecisionEntry, req: &DecisionLogRequest, project: Option<&str>) -> bool {
    if project.is_some_and(|handle| entry.project != handle) {
        return false;
    }
    if let Some(since) = req.since {
        match entry.occurred_at {
            Some(ts) if ts >= since => {}
            _ => return false,
        }
    }
    if !req.topics.is_empty() && !req.topics.iter().any(|t| entry.topic_keys.contains(t)) {
        return false;
    }
    if !req.statuses.is_empty() && !req.statuses.contains(&entry.status) {
        return false;
    }
    true
}

// ---------------------------------------------------------------------------
// Small graph sub-queries not already exposed by sibling query modules
// ---------------------------------------------------------------------------

/// Actors connected to this decision via `relation` (`AcceptedBy` / `RejectedBy`), sorted by
/// id. Built on the same generic one-hop helper `neighborhood.rs` and `compact_view.rs` use.
fn actor_ids(
    graph: &impl GraphView,
    decision_id: &str,
    relation: RelationKind,
) -> Result<Vec<String>> {
    Ok(neighbor_pairs(
        graph,
        NodeKind::Decision,
        decision_id,
        relation,
        NodeKind::Actor,
        Direction::Outgoing,
    )?
    .into_iter()
    .map(|(actor_id, _)| actor_id)
    .collect())
}

fn last_event_timestamp(
    ledger: &impl EventLedger,
    latest_offset: EventId,
) -> Result<Option<DateTime<Utc>>> {
    if latest_offset == 0 {
        return Ok(None);
    }
    let events = ledger.read(latest_offset.saturating_sub(1), 1)?;
    Ok(events.into_iter().next().and_then(|event| event.ts))
}

// ---------------------------------------------------------------------------
// Filenames
// ---------------------------------------------------------------------------

fn assign_filenames(entries: &[DecisionEntry]) -> BTreeMap<String, String> {
    let mut filenames = BTreeMap::new();
    for entry in entries {
        let mut filename = String::new();
        match entry.occurred_at {
            Some(ts) => {
                let _ = write!(filename, "{}", ts.format("%Y-%m-%d"));
            }
            None => filename.push_str("undated"),
        }
        let slug = capped_slug(&entry.title);
        if !slug.is_empty() {
            filename.push('-');
            filename.push_str(&slug);
        }
        filename.push('-');
        filename.push_str(&short_id(&entry.id));
        filename.push_str(".md");
        filenames.insert(entry.id.clone(), filename); // ubs:ignore: clone necessary — owned key for the filenames map, entry stays borrowed for later iterations
    }
    filenames
}

fn capped_slug(title: &str) -> String {
    let mut slug = normalize_topic_key(title);
    if slug.len() > MAX_SLUG_LEN {
        slug.truncate(MAX_SLUG_LEN);
        while slug.ends_with('-') {
            slug.pop();
        }
    }
    slug
}

/// Ids are `decision-<uuid>` in production (`generate_entity_id`, src/commands/mod.rs), short
/// sequential ids (`decision-005`) in `tests/support/seed_data.rs`, and colon-namespaced ids
/// (`org:incident:decision:declare`) in `tests/support/organizational_scenarios.rs`. Stripping
/// the known prefix and keeping only filename-safe characters gives the UUID's first hex group
/// in production and a short, still-unique fragment in every fixture shape — enough to
/// disambiguate a same-day, same-title collision since the full id is already globally unique.
fn short_id(decision_id: &str) -> String {
    let stripped = decision_id.strip_prefix("decision-").unwrap_or(decision_id);
    stripped
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(ID8_LEN)
        .collect()
}

// ---------------------------------------------------------------------------
// Projects and paths
// ---------------------------------------------------------------------------

/// One project's place in the export: where its files go and what its pages are called.
struct ProjectSlot {
    handle: String,
    /// Directory of the project's files, relative to the export root.
    dir: String,
    /// Heading of the project's page and of its section in the root index.
    label: String,
}

impl ProjectSlot {
    fn shared(handle: &str, display_name: Option<&str>) -> Self {
        Self {
            handle: handle.to_owned(),
            dir: format!("{PROJECTS_DIR}/{handle}"),
            label: match display_name {
                Some(display_name) => format!("{display_name} ({handle})"),
                None => handle.to_owned(),
            },
        }
    }

    /// `slug` is the address's actor part reduced to path-safe characters; `ambiguous` appends
    /// a short hash of the full address so the directory cannot collide with another slot's.
    fn personal(handle: &str, slug: &str, ambiguous: bool) -> Self {
        let dir_name = if ambiguous {
            format!("{slug}-{}", short_hash(handle))
        } else {
            slug.to_owned()
        };
        Self {
            handle: handle.to_owned(),
            dir: format!("{PROJECTS_DIR}/{PERSONAL_DIR}/{dir_name}"),
            label: format!(
                "Personal project: {}",
                handle
                    .strip_prefix(PERSONAL_PROJECT_HANDLE_PREFIX)
                    .unwrap_or(handle)
            ),
        }
    }

    fn decisions_dir(&self) -> String {
        format!("{}/{DECISIONS_DIR}", self.dir)
    }

    fn index_path(&self) -> String {
        format!("{}/{INDEX_FILE}", self.dir)
    }
}

/// Places every project in the export, shared projects first (by handle), then personal
/// ones. A shared handle (lowercase letters, digits, dashes) is already path-safe and lives
/// at `projects/<handle>`; a personal address lives at `projects/personal/<actor>`, the
/// actor part reduced to path-safe characters. Personal addresses whose reduced actor part
/// coincides — or equals `decisions`, the name a shared project called `personal` uses for
/// its decision files — each get a short hash of their full address appended, so two
/// people's personal projects never write over each other.
fn assign_projects(
    handles: &BTreeSet<&str>,
    registered: &BTreeMap<String, Option<String>>,
) -> Vec<ProjectSlot> {
    let mut slots = Vec::with_capacity(handles.len());
    let mut personal: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for &handle in handles {
        match handle.strip_prefix(PERSONAL_PROJECT_HANDLE_PREFIX) {
            Some(actor) => personal.entry(path_safe(actor)).or_default().push(handle),
            None => slots.push(ProjectSlot::shared(
                handle,
                registered.get(handle).and_then(Option::as_deref),
            )),
        }
    }

    let mut personal_slots = Vec::new();
    for (slug, group) in &personal {
        let ambiguous = group.len() > 1 || slug == DECISIONS_DIR;
        for &handle in group {
            personal_slots.push(ProjectSlot::personal(handle, slug, ambiguous));
        }
    }
    personal_slots.sort_by(|a, b| a.handle.cmp(&b.handle));
    slots.extend(personal_slots);
    slots
}

/// Reduces `text` to `[A-Za-z0-9_-]`: `:` in `human:alex`, and the `@` and `.` of an email
/// actor, become `-`. One safe path segment on every platform, never `.` or `..`.
fn path_safe(text: &str) -> String {
    let slug: String = text
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if slug.is_empty() {
        "unnamed".to_owned()
    } else {
        slug
    }
}

fn short_hash(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .take(4)
        .fold(String::new(), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

fn members_of<'a>(
    by_project: &'a BTreeMap<&str, Vec<&'a DecisionEntry>>,
    project: &ProjectSlot,
) -> &'a [&'a DecisionEntry] {
    by_project
        .get(project.handle.as_str())
        .map_or(&[], Vec::as_slice)
}

fn exported_filename<'a>(filenames: &'a BTreeMap<String, String>, decision_id: &str) -> &'a str {
    filenames // ubs:ignore: expect below documents a construction invariant (assign_filenames covers every entry in this same slice), not a real panic risk
        .get(decision_id)
        .expect("filename assigned for every exported entry, populated just above")
}

fn decision_path(project: &ProjectSlot, filename: &str) -> String {
    format!("{}/{DECISIONS_DIR}/{filename}", project.dir)
}

/// Markdown link target from a file in `from_dir` to `to_path`, both relative to the export
/// root and `/`-separated (`""` is the root). A decision's supersession neighbours may
/// belong to another project, so links are computed, never assumed to sit next to each other.
fn relative_link(from_dir: &str, to_path: &str) -> String {
    let from: Vec<&str> = from_dir
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    let to: Vec<&str> = to_path.split('/').collect();
    let common = from
        .iter()
        .zip(&to)
        .take_while(|(from_part, to_part)| from_part == to_part)
        .count()
        .min(to.len().saturating_sub(1));
    let mut link = "../".repeat(from.len().saturating_sub(common));
    write_joined(&mut link, to.iter().skip(common), "/");
    link
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn status_word(status: DecisionStatus) -> &'static str {
    match status {
        DecisionStatus::Proposed => "proposed",
        DecisionStatus::Accepted => "accepted",
        DecisionStatus::Rejected => "rejected",
        DecisionStatus::Contested => "contested",
        DecisionStatus::Superseded => "superseded",
    }
}

/// Link text/target for a cross-referenced decision: a Markdown link when it is in the
/// exported set (has a path), otherwise its title and id as plain text — never a dead
/// link (design Q1/Q5 in hivemind-xw61's DESIGN). `from_dir` is the directory of the file
/// the link is written into, so a neighbour in another project still resolves.
fn render_decision_ref(
    decision_id: &str,
    paths: &BTreeMap<String, String>,
    titles: &BTreeMap<String, String>,
    from_dir: &str,
) -> String {
    let title = titles
        .get(decision_id)
        .cloned()
        .unwrap_or_else(|| decision_id.to_owned());
    match paths.get(decision_id) {
        Some(path) => format!("[{title}]({})", relative_link(from_dir, path)),
        None => format!("{title} ({decision_id})"),
    }
}

fn render_decision_file(
    entry: &DecisionEntry,
    paths: &BTreeMap<String, String>,
    titles: &BTreeMap<String, String>,
    from_dir: &str,
) -> String {
    let front_matter = render_front_matter(entry);
    let mut body = String::new();
    let _ = write!(body, "# {}\n\n", entry.title);
    body.push_str(&render_status_line(entry, paths, titles, from_dir));
    body.push_str("\n\n## Context\n\n");
    body.push_str(&render_context_section(entry));
    body.push_str("\n\n## Options considered\n\n");
    body.push_str(&render_options_section(entry));
    body.push_str("\n\n## Decision\n\n");
    body.push_str(&render_decision_section(entry));
    body.push_str("\n\n## Rests on\n\n");
    body.push_str(&render_rests_on_section(entry, paths, titles, from_dir));
    body.push_str("\n\n## Evidence\n\n");
    body.push_str(&render_evidence_section(entry));
    body.push_str("\n\n## Outcome\n\n");
    body.push_str(&render_outcome_section(entry, paths, titles, from_dir));
    body.push_str("\n\n## Provenance\n\n");
    body.push_str(&render_provenance_section(entry));
    body.push('\n');

    format!("{front_matter}\n\n{body}")
}

fn yaml_scalar(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n");
    format!("\"{escaped}\"")
}

fn yaml_optional_scalar(value: Option<&str>) -> String {
    value.map_or_else(|| "null".to_owned(), yaml_scalar)
}

fn yaml_list(values: &[String]) -> String {
    if values.is_empty() {
        "[]".to_owned()
    } else {
        let items: Vec<String> = values.iter().map(|v| yaml_scalar(v)).collect();
        format!("[{}]", items.join(", "))
    }
}

fn render_front_matter(entry: &DecisionEntry) -> String {
    let occurred_at = entry
        .occurred_at
        .map(|ts| yaml_scalar(&ts.to_rfc3339()))
        .unwrap_or_else(|| "null".to_owned());
    let event_origin = entry
        .event_origin
        .map(|v| v.to_string())
        .unwrap_or_else(|| "null".to_owned());
    let lines = [
        format!("id: {}", yaml_scalar(&entry.id)),
        format!("title: {}", yaml_scalar(&entry.title)),
        format!("status: {}", yaml_scalar(status_word(entry.status))),
        format!("occurred_at: {occurred_at}"),
        format!("topic_keys: {}", yaml_list(&entry.topic_keys)),
        format!(
            "proposer: {}",
            yaml_optional_scalar(entry.proposer_id.as_deref())
        ),
        format!("source: {}", yaml_scalar(&entry.source)),
        format!(
            "source_ref: {}",
            yaml_optional_scalar(entry.source_ref.as_deref())
        ),
        format!("event_origin: {event_origin}"),
        format!(
            "supersedes: {}",
            yaml_optional_scalar(entry.supersedes.as_deref())
        ),
        format!(
            "superseded_by: {}",
            yaml_optional_scalar(entry.all_superseded_by.first().map(String::as_str))
        ),
    ];
    format!("---\n{}\n---", lines.join("\n"))
}

fn render_status_line(
    entry: &DecisionEntry,
    paths: &BTreeMap<String, String>,
    titles: &BTreeMap<String, String>,
    from_dir: &str,
) -> String {
    match entry.status {
        DecisionStatus::Superseded if !entry.all_superseded_by.is_empty() => {
            let refs: Vec<String> = entry
                .all_superseded_by
                .iter()
                .map(|id| render_decision_ref(id, paths, titles, from_dir))
                .collect();
            format!("Status: superseded → {}", refs.join(", "))
        }
        DecisionStatus::Superseded => "Status: superseded".to_owned(),
        DecisionStatus::Contested => format!(
            "Status: contested — accepted by: {}; rejected by: {}",
            render_actor_list(&entry.accepted_by),
            render_actor_list(&entry.rejected_by),
        ),
        other => format!("Status: {}", status_word(other)),
    }
}

fn render_actor_list(actors: &[String]) -> String {
    if actors.is_empty() {
        "none".to_owned()
    } else {
        actors.join(", ")
    }
}

fn render_context_section(entry: &DecisionEntry) -> String {
    let mut out = String::new();
    if entry.topic_keys.is_empty() {
        out.push_str("Topic keys: None recorded.");
    } else {
        out.push_str("Topic keys: ");
        write_joined(&mut out, entry.topic_keys.iter(), ", ");
    }

    out.push_str("\n\nHypotheses premised on:");
    if entry.hypotheses.is_empty() {
        out.push_str(" None recorded.");
    } else {
        for hyp in &entry.hypotheses {
            let status_text = match hyp.status {
                HypothesisStatus::Refuted => "**refuted**",
                HypothesisStatus::Supported => "supported",
                HypothesisStatus::Open => "open",
            };
            let _ = write!(out, "\n- {} ({}): {status_text}", hyp.statement, hyp.id);
        }
    }
    out
}

fn render_options_section(entry: &DecisionEntry) -> String {
    if entry.chosen_option.is_none() && entry.rejected_options.is_empty() {
        return "None recorded.".to_owned();
    }
    let mut out = String::new();
    let mut first = true;
    if let Some(chosen) = &entry.chosen_option {
        let _ = write!(out, "- **{}** (chosen)", chosen.label);
        first = false;
    }
    for option in &entry.rejected_options {
        if !first {
            out.push('\n');
        }
        let _ = write!(out, "- {}", option.label);
        first = false;
    }
    out
}

fn render_decision_section(entry: &DecisionEntry) -> String {
    let base = match &entry.chosen_option {
        Some(chosen) => format!("**{}**\n\n{}", chosen.label, entry.rationale),
        None if entry.rationale.trim().is_empty() => "None recorded.".to_owned(),
        None => entry.rationale.clone(), // ubs:ignore: clone necessary — other match arms return owned String, entry stays borrowed
    };
    match (&entry.question, &entry.quote) {
        (Some(question), Some(quote)) => {
            format!("{base}\n\n**Answers:** {question}\n\n**Quote:** \"{quote}\"")
        }
        _ => base,
    }
}

/// What the decision rests on: one line per item with its state and whether it was named at
/// capture or attributed later, an honest "nothing declared" for a decision nobody asked, and
/// how many decisions rest on this one.
fn render_rests_on_section(
    entry: &DecisionEntry,
    paths: &BTreeMap<String, String>,
    titles: &BTreeMap<String, String>,
    from_dir: &str,
) -> String {
    let mut out = String::new();
    if entry.grounding_state == GroundingState::NothingDeclared {
        out.push_str("Nothing declared (never asked).");
    } else {
        for (i, item) in entry.rests_on.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            let (kind, label) = match item.kind {
                GroundingKind::Decision => (
                    "Decision",
                    render_decision_ref(&item.id, paths, titles, from_dir),
                ),
                GroundingKind::Evidence => ("Evidence", item.label.clone()), // ubs:ignore: clone necessary — the item is borrowed from the entry
                GroundingKind::Assumption => ("Assumption", item.label.clone()), // ubs:ignore: clone necessary — the item is borrowed from the entry
                GroundingKind::Bet => ("Bet", item.label.clone()), // ubs:ignore: clone necessary — the item is borrowed from the entry
            };
            let _ = write!(out, "- **{kind}:** {label}");
            if let Some(state) = item.state.describe() {
                let _ = write!(out, " — {state}");
            }
            let _ = write!(out, " ({})", item.added.describe());
        }
    }
    if let Some(confidence) = &entry.expressed_confidence {
        let _ = write!(
            out,
            "\n\nConfidence at capture: {confidence} (decider's words)"
        );
    }
    if entry.dependents_count > 0 {
        let noun = if entry.dependents_count == 1 {
            "decision rests"
        } else {
            "decisions rest"
        };
        let _ = write!(out, "\n\n{} {noun} on this.", entry.dependents_count);
    }
    out
}

fn render_evidence_section(entry: &DecisionEntry) -> String {
    if entry.evidence.is_empty() {
        return "None recorded.".to_owned();
    }
    let mut out = String::new();
    for (i, evidence) in entry.evidence.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        match &evidence.source_ref {
            Some(source_ref) if looks_like_url(source_ref) => {
                let _ = write!(
                    out,
                    "- {} (source: [{source_ref}]({source_ref}))",
                    evidence.content
                );
            }
            Some(source_ref) => {
                let _ = write!(out, "- {} (source: {source_ref})", evidence.content);
            }
            None => {
                let _ = write!(out, "- {} (source: None recorded.)", evidence.content);
            }
        }
    }
    out
}

fn looks_like_url(value: &str) -> bool {
    value.starts_with("http://") || value.starts_with("https://")
}

fn render_outcome_reason(reason: &OutcomeReason) -> String {
    match reason {
        OutcomeReason::SupersededBy { by_id, gap_events } => match gap_events {
            Some(gap) => format!("Superseded by {by_id} ({gap} ledger events later)"),
            None => format!("Superseded by {by_id}"),
        },
        OutcomeReason::PremisedOnRefuted { hypothesis_id } => {
            format!("Premised on refuted hypothesis {hypothesis_id}")
        }
        OutcomeReason::PremiseSuperseded { decision_id, by_id } => {
            format!("Follows from {decision_id}, which was superseded by {by_id}")
        }
        OutcomeReason::PremiseRejected { decision_id } => {
            format!("Follows from {decision_id}, which was rejected")
        }
        OutcomeReason::Contested => "Contested".to_owned(),
        OutcomeReason::ThinStructure {
            no_options,
            nothing_declared,
        } => {
            let mut parts = Vec::new();
            if *no_options {
                parts.push("no options");
            }
            if *nothing_declared {
                parts.push("nothing declared about what it rests on");
            }
            format!("Thin structure: {}", parts.join(" and "))
        }
    }
}

fn render_outcome_section(
    entry: &DecisionEntry,
    paths: &BTreeMap<String, String>,
    titles: &BTreeMap<String, String>,
    from_dir: &str,
) -> String {
    let mut out = String::new();
    let _ = write!(
        out,
        "Still holds: **{}**",
        if entry.held_up { "yes" } else { "no" }
    );

    if entry.outcome_reasons.is_empty() {
        out.push_str("\nReasons: None recorded.");
    } else {
        out.push_str("\nReasons:");
        for reason in &entry.outcome_reasons {
            out.push_str("\n- ");
            out.push_str(&render_outcome_reason(reason));
        }
    }

    out.push_str("\n\nSupersedes: ");
    match entry.supersedes.as_deref() {
        Some(id) => out.push_str(&render_decision_ref(id, paths, titles, from_dir)),
        None => out.push_str("None recorded."),
    }

    out.push_str("\nSuperseded by: ");
    if entry.all_superseded_by.is_empty() {
        out.push_str("None recorded.");
    } else {
        write_joined(
            &mut out,
            entry
                .all_superseded_by
                .iter()
                .map(|id| render_decision_ref(id, paths, titles, from_dir)),
            ", ",
        );
    }

    out.push_str("\n\nActive blockers:");
    if entry.active_blockers.is_empty() {
        out.push_str(" None recorded.");
    } else {
        for blocker in &entry.active_blockers {
            let _ = write!(
                out,
                "\n- {} ({}, blocking {}): {}",
                blocker.id,
                blocker.priority.as_str(),
                blocker.blocked_actor_id,
                blocker.reason
            );
        }
    }

    out
}

fn render_provenance_section(entry: &DecisionEntry) -> String {
    let lines = [
        format!(
            "- Event origin: {}",
            entry
                .event_origin
                .map(|v| v.to_string())
                .unwrap_or_else(|| "None recorded.".to_owned())
        ),
        format!("- Source: {}", entry.source),
        format!(
            "- Source ref: {}",
            entry.source_ref.as_deref().unwrap_or("None recorded.")
        ),
        format!(
            "- Proposer: {}",
            entry.proposer_id.as_deref().unwrap_or("None recorded.")
        ),
        format!(
            "- Accepted by: {}",
            render_actor_list_or_none(&entry.accepted_by)
        ),
        format!(
            "- Rejected by: {}",
            render_actor_list_or_none(&entry.rejected_by)
        ),
        format!(
            "- Ledger timestamp: {}",
            entry
                .occurred_at
                .map(|ts| ts.to_rfc3339())
                .unwrap_or_else(|| "None recorded.".to_owned())
        ),
    ];
    lines.join("\n")
}

fn render_actor_list_or_none(actors: &[String]) -> String {
    if actors.is_empty() {
        "None recorded.".to_owned()
    } else {
        actors.join(", ")
    }
}

fn render_filters(req: &DecisionLogRequest) -> String {
    let since_text = req
        .since
        .map(|ts| ts.to_rfc3339())
        .unwrap_or_else(|| "none".to_owned());
    let topics_text = if req.topics.is_empty() {
        "any".to_owned()
    } else {
        req.topics.join(",")
    };
    let statuses_text = if req.statuses.is_empty() {
        "any".to_owned()
    } else {
        req.statuses
            .iter()
            .map(|status| status_word(*status))
            .collect::<Vec<_>>()
            .join(",")
    };
    let project_text = req.project.as_deref().map_or("any", str::trim);
    format!(
        "project={project_text}  topic={topics_text}  status={statuses_text}  since={since_text}"
    )
}

fn render_generated_line(
    out: &mut String,
    ledger_offset: EventId,
    last_event_ts: Option<DateTime<Utc>>,
) {
    let last_event_text = last_event_ts
        .map(|ts| ts.to_rfc3339())
        .unwrap_or_else(|| "none".to_owned());
    let _ = writeln!(
        out,
        "Generated from HiveMind ledger offset {ledger_offset} (last event {last_event_text}). Do not edit; regenerate with `hivemind export`.\n"
    );
}

fn render_counts<'a>(out: &mut String, entries: impl IntoIterator<Item = &'a &'a DecisionEntry>) {
    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    for entry in entries {
        *counts.entry(status_word(entry.status)).or_insert(0) += 1;
    }
    if counts.is_empty() {
        out.push_str("Counts: none.\n\n");
    } else {
        let parts: Vec<String> = counts
            .iter()
            .map(|(status, count)| format!("{status}={count}"))
            .collect();
        let _ = writeln!(out, "Counts: {}\n", parts.join(", "));
    }
}

/// The root `INDEX.md`: every project in the export as its own section, each with a link to
/// the project's own record and the table of its decisions. Reading top to bottom is the
/// whole tenant; `projects/<dir>/INDEX.md` is one project's record on its own.
fn render_root_index(
    projects: &[ProjectSlot],
    by_project: &BTreeMap<&str, Vec<&DecisionEntry>>,
    req: &DecisionLogRequest,
    ledger_offset: EventId,
    last_event_ts: Option<DateTime<Utc>>,
    paths: &BTreeMap<String, String>,
    titles: &BTreeMap<String, String>,
) -> String {
    let mut out = String::new();
    out.push_str("# Decision log\n\n");
    render_generated_line(&mut out, ledger_offset, last_event_ts);
    let _ = writeln!(out, "Filters: {}\n", render_filters(req));
    render_counts(&mut out, by_project.values().flatten());

    for project in projects {
        let members = members_of(by_project, project);
        if !out.ends_with("\n\n") {
            out.push('\n');
        }
        let _ = write!(out, "## {}\n\n", project.label);
        let _ = writeln!(
            out,
            "Project record: [{dir}/{INDEX_FILE}]({dir}/{INDEX_FILE})\n",
            dir = project.dir
        );
        render_counts(&mut out, members);
        render_index_table(&mut out, members, paths, titles, "");
    }

    out
}

/// One project's own `projects/<dir>/INDEX.md`: self-contained (header, filters, counts,
/// table) so the directory can be read, or copied out, on its own.
fn render_project_index(
    project: &ProjectSlot,
    members: &[&DecisionEntry],
    req: &DecisionLogRequest,
    ledger_offset: EventId,
    last_event_ts: Option<DateTime<Utc>>,
    paths: &BTreeMap<String, String>,
    titles: &BTreeMap<String, String>,
) -> String {
    let mut out = String::new();
    let _ = write!(out, "# {}\n\n", project.label);
    render_generated_line(&mut out, ledger_offset, last_event_ts);
    let _ = writeln!(
        out,
        "Project: `{}` · [All projects]({})\n",
        project.handle,
        relative_link(&project.dir, INDEX_FILE)
    );
    let _ = writeln!(out, "Filters: {}\n", render_filters(req));
    render_counts(&mut out, members);
    render_index_table(&mut out, members, paths, titles, &project.dir);
    out
}

/// Newest-first table of `members`, linked relative to `from_dir` (the directory of the
/// index the table is written into). Writes nothing for an empty project: the `Counts: none.`
/// line above it already says so.
fn render_index_table(
    out: &mut String,
    members: &[&DecisionEntry],
    paths: &BTreeMap<String, String>,
    titles: &BTreeMap<String, String>,
    from_dir: &str,
) {
    if members.is_empty() {
        return;
    }
    out.push_str("| Date | Decision | Status | Topics | Decided by |\n");
    out.push_str("|---|---|---|---|---|\n");

    let mut newest_first: Vec<&&DecisionEntry> = members.iter().collect();
    newest_first.sort_by(|a, b| {
        (b.occurred_at, b.event_origin, &b.id).cmp(&(a.occurred_at, a.event_origin, &a.id))
    });

    for entry in newest_first {
        out.push_str("| ");
        match entry.occurred_at {
            Some(ts) => {
                let _ = write!(out, "{}", ts.format("%Y-%m-%d"));
            }
            None => out.push_str("undated"),
        }
        let _ = write!(
            out,
            " | {} | ",
            render_decision_ref(&entry.id, paths, titles, from_dir)
        );
        match entry.status {
            DecisionStatus::Superseded if !entry.all_superseded_by.is_empty() => {
                out.push_str("superseded → ");
                write_joined(
                    out,
                    entry
                        .all_superseded_by
                        .iter()
                        .map(|id| render_decision_ref(id, paths, titles, from_dir)),
                    ", ",
                );
            }
            DecisionStatus::Superseded => out.push_str("superseded"),
            other => out.push_str(status_word(other)),
        }
        out.push_str(" | ");
        write_joined(out, entry.topic_keys.iter(), ", ");
        out.push_str(" | ");
        if entry.accepted_by.is_empty() {
            out.push_str(entry.proposer_id.as_deref().unwrap_or_default());
        } else {
            write_joined(out, entry.accepted_by.iter(), ", ");
        }
        out.push_str(" |\n");
    }
}

/// Writes `items` into `out` separated by `sep`, without the intermediate `Vec<String>` and
/// `String` that `.collect::<Vec<_>>().join(sep)` would allocate.
fn write_joined<S: AsRef<str>>(out: &mut String, items: impl IntoIterator<Item = S>, sep: &str) {
    for (i, item) in items.into_iter().enumerate() {
        if i > 0 {
            out.push_str(sep);
        }
        out.push_str(item.as_ref());
    }
}

#[cfg(test)]
mod tests;
