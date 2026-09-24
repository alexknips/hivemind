//! Verb-level grounding shared by the CLI, MCP and REST capture verbs: "what does this
//! decision rest on?" (hivemind-gwhr.2).
//!
//! A capture verb collects what its caller said the decision rests on into a `GroundingSpec`,
//! this module resolves the named premise decisions against the graph (the query layer's
//! resolver: deterministic, ambiguity-gated, no ranking of its own) and returns the
//! `GroundingPlan` the commands layer enforces and records. Nothing here writes to the ledger,
//! and nothing here decides *which* decision a vague description meant — an ambiguous or
//! unmatched premise is handed back to the caller untouched, and the verb refuses to write.
//!
//! The wire form (`grounding` array items, shared by MCP and REST) is parsed here so both
//! transports reject the same malformed input with the same text.

use serde_json::{json, Map, Value};

use crate::commands::{GroundingPlan, NewBet, NewEvidence, RestsOn, RestsOnKind};
use crate::events::TenantId;
use crate::ledger::EventLedger;
use crate::projector::{memory::MemoryGraph, rebuild_graph_for_tenant};
use crate::queries::{
    resolve_decision_by_description, resolve_decision_by_id, QueryResponse, ResolveOutcome,
    ResolvedCandidate,
};
use crate::util::parse_check_by;
use crate::Result;

/// The four ways to answer "what does this decision rest on?", as the MCP and REST refusals
/// quote them. The CLI's refusal names its flags instead (`cli::run`).
pub(crate) const WIRE_GROUNDING_REFUSAL: &str = "a captured decision must say what it rests on: pass `grounding` with at least one item — {kind:\"decision\", description|decision_id} (a decision already made), {kind:\"evidence\", content, source?} (something observed, and where), {kind:\"assumption\", statement} (something assumed), or {kind:\"bet\", statement?, would_change_if?, check_by?} (nothing yet: a declared bet)";

/// How a premise decision is named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PremiseTarget {
    /// A literal decision id.
    Id(String),
    /// Free text, resolved by the tenv resolver and its ambiguity gate.
    Description(String),
}

/// One premise decision the caller named, with the name of the field it came from so a refusal
/// can point at it (`grounding[2]`, `--rests-on-decision`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PremiseSpec {
    pub(crate) field: String,
    pub(crate) target: PremiseTarget,
}

/// What a caller said the decision rests on, before any premise is resolved.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct GroundingSpec {
    pub(crate) decisions: Vec<PremiseSpec>,
    pub(crate) evidence: Vec<NewEvidence>,
    pub(crate) evidence_ids: Vec<String>,
    pub(crate) assumptions: Vec<String>,
    pub(crate) hypothesis_ids: Vec<String>,
    pub(crate) bet: Option<NewBet>,
}

impl GroundingSpec {
    pub(crate) fn is_empty(&self) -> bool {
        self.decisions.is_empty()
            && self.evidence.is_empty()
            && self.evidence_ids.is_empty()
            && self.assumptions.is_empty()
            && self.hypothesis_ids.is_empty()
            && self.bet.is_none()
    }

    /// Parse the wire `grounding` array (MCP and REST) plus the deprecated `hypothesis_ids` /
    /// `evidence_ids` aliases, which map into the same spec: an id listed there counts as
    /// grounding exactly like `{kind:"assumption", hypothesis_id}` / `{kind:"evidence",
    /// evidence_id}`.
    pub(crate) fn from_wire(
        items: &[Value],
        hypothesis_id_aliases: &[String],
        evidence_id_aliases: &[String],
    ) -> std::result::Result<Self, String> {
        let mut spec = Self::default();
        for (index, item) in items.iter().enumerate() {
            spec.push_wire_item(index, item)?;
        }
        spec.hypothesis_ids
            .extend(hypothesis_id_aliases.iter().cloned());
        spec.evidence_ids
            .extend(evidence_id_aliases.iter().cloned());
        Ok(spec)
    }

    fn push_wire_item(&mut self, index: usize, item: &Value) -> std::result::Result<(), String> {
        let field = format!("grounding[{index}]");
        let Value::Object(map) = item else {
            return Err(format!("`{field}` must be an object"));
        };
        let kind = match map.get("kind") {
            Some(Value::String(kind)) => kind.as_str(),
            _ => {
                return Err(format!(
                    "`{field}.kind` must be one of decision, evidence, assumption, bet"
                ))
            }
        };
        let allowed: &[&str] = match kind {
            "decision" => &["kind", "description", "decision_id"],
            "evidence" => &["kind", "content", "source", "evidence_id"],
            "assumption" => &["kind", "statement", "hypothesis_id"],
            "bet" => &["kind", "statement", "would_change_if", "check_by"],
            other => {
                return Err(format!(
                    "`{field}.kind` `{other}` is not one of decision, evidence, assumption, bet"
                ))
            }
        };
        if let Some(unknown) = map.keys().find(|key| !allowed.contains(&key.as_str())) {
            return Err(format!(
                "`{field}` has unknown field `{unknown}` for kind `{kind}`"
            ));
        }

        match kind {
            "decision" => {
                let description = wire_text(map, &field, "description")?;
                let decision_id = wire_text(map, &field, "decision_id")?;
                let target = match (description, decision_id) {
                    (Some(text), None) => PremiseTarget::Description(text),
                    (None, Some(id)) => PremiseTarget::Id(id),
                    _ => {
                        return Err(format!(
                            "`{field}` of kind `decision` needs exactly one of `description` or `decision_id`"
                        ))
                    }
                };
                self.decisions.push(PremiseSpec { field, target });
            }
            "evidence" => {
                let content = wire_text(map, &field, "content")?;
                let source = wire_text(map, &field, "source")?;
                let evidence_id = wire_text(map, &field, "evidence_id")?;
                match (content, evidence_id) {
                    (Some(content), None) => self.evidence.push(NewEvidence { content, source }),
                    (None, Some(id)) if source.is_none() => self.evidence_ids.push(id),
                    (None, Some(_)) => {
                        return Err(format!(
                            "`{field}` names an existing `evidence_id`, which has a source already; drop `source`"
                        ))
                    }
                    _ => {
                        return Err(format!(
                            "`{field}` of kind `evidence` needs exactly one of `content` (new evidence, with an optional `source`) or `evidence_id`"
                        ))
                    }
                }
            }
            "assumption" => {
                let statement = wire_text(map, &field, "statement")?;
                let hypothesis_id = wire_text(map, &field, "hypothesis_id")?;
                match (statement, hypothesis_id) {
                    (Some(statement), None) => self.assumptions.push(statement),
                    (None, Some(id)) => self.hypothesis_ids.push(id),
                    _ => {
                        return Err(format!(
                            "`{field}` of kind `assumption` needs exactly one of `statement` or `hypothesis_id`"
                        ))
                    }
                }
            }
            _ => {
                if self.bet.is_some() {
                    return Err(format!(
                        "`{field}`: a decision declares at most one bet; put what would change your mind in `would_change_if`"
                    ));
                }
                let check_by = wire_text(map, &field, "check_by")?
                    .map(|value| {
                        parse_check_by(&value).ok_or_else(|| {
                            format!(
                                "`{field}.check_by` must be an RFC3339 timestamp or a YYYY-MM-DD date (got: {value})"
                            )
                        })
                    })
                    .transpose()?;
                self.bet = Some(NewBet {
                    statement: wire_text(map, &field, "statement")?,
                    would_change_if: wire_text(map, &field, "would_change_if")?,
                    check_by,
                });
            }
        }
        Ok(())
    }
}

/// A non-empty string field, or `None` when absent or null. Blank strings and non-strings are
/// errors: a blank premise is never silently dropped.
fn wire_text(
    map: &Map<String, Value>,
    field: &str,
    key: &str,
) -> std::result::Result<Option<String>, String> {
    match map.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) if !text.trim().is_empty() => Ok(Some(text.clone())),
        Some(Value::String(_)) => Err(format!("`{field}.{key}` must not be blank")),
        Some(_) => Err(format!("`{field}.{key}` must be a string")),
    }
}

/// A premise the resolver could not turn into exactly one decision.
#[derive(Debug, Clone)]
pub(crate) struct UnresolvedPremise {
    pub(crate) field: String,
    /// The premise as the caller wrote it (description or id).
    pub(crate) text: String,
    /// True when the caller named a literal id rather than a description.
    pub(crate) by_id: bool,
    /// `Ambiguous` or `NotFound`, never `Resolved`.
    pub(crate) response: QueryResponse<ResolveOutcome>,
}

impl UnresolvedPremise {
    pub(crate) fn candidates(&self) -> &[ResolvedCandidate] {
        match &self.response.data {
            ResolveOutcome::Ambiguous { candidates } => candidates,
            _ => &[],
        }
    }

    /// The MCP / REST success-shaped result for an unresolved premise: the resolver envelope
    /// every fluent tool returns, with the offending field named. No event was appended.
    pub(crate) fn envelope(&self) -> Value {
        let mut data = Map::new();
        data.insert("field".to_owned(), json!(self.field));
        match &self.response.data {
            ResolveOutcome::Ambiguous { candidates } => {
                data.insert("outcome".to_owned(), json!("ambiguous"));
                data.insert("candidates".to_owned(), json!(candidates));
            }
            _ => {
                let key = if self.by_id {
                    "decision_id"
                } else {
                    "description"
                };
                data.insert("outcome".to_owned(), json!("not_found"));
                data.insert(key.to_owned(), json!(self.text));
            }
        }
        json!({
            "result_count": self.response.result_count,
            "truncated": self.response.truncated,
            "latency_ms": self.response.latency_ms,
            "data": data,
        })
    }
}

/// A spec with every premise resolved: the plan to hand the commands, and the title of each
/// premise decision for the reply.
#[derive(Debug, Clone)]
pub(crate) struct ResolvedGrounding {
    pub(crate) plan: GroundingPlan,
    /// One title per `plan.premise_decision_ids` entry, in the same order.
    premise_titles: Vec<String>,
}

impl ResolvedGrounding {
    /// Fill in the titles of premise decisions in a `rests_on` the commands returned.
    pub(crate) fn label(&self, mut rests_on: Vec<RestsOn>) -> Vec<RestsOn> {
        for item in &mut rests_on {
            if item.kind == RestsOnKind::Decision && item.label.is_none() {
                item.label = self
                    .plan
                    .premise_decision_ids
                    .iter()
                    .position(|premise_id| *premise_id == item.id)
                    .and_then(|index| self.premise_titles.get(index))
                    .cloned();
            }
        }
        rests_on
    }
}

pub(crate) enum GroundingResolution {
    Ready(ResolvedGrounding),
    Unresolved(UnresolvedPremise),
}

/// Resolve every premise decision in `spec`. Stops at the first premise that is ambiguous or
/// matches nothing — the caller fixes it and re-runs, so `#N` continuation (CLI) only ever has
/// one candidate list to point at. The graph is rebuilt only when a premise decision is named.
pub(crate) fn resolve_grounding<L: EventLedger>(
    ledger: &L,
    tenant_id: &TenantId,
    spec: GroundingSpec,
) -> Result<GroundingResolution> {
    let graph = MemoryGraph::default();
    if !spec.decisions.is_empty() {
        rebuild_graph_for_tenant(ledger, tenant_id, &graph)?;
    }

    let mut premise_decision_ids: Vec<String> = Vec::with_capacity(spec.decisions.len());
    let mut premise_titles: Vec<String> = Vec::with_capacity(spec.decisions.len());
    for PremiseSpec { field, target } in spec.decisions {
        let (text, by_id) = match target {
            PremiseTarget::Id(id) => (id, true),
            PremiseTarget::Description(text) => (text, false),
        };
        let response = if by_id {
            resolve_decision_by_id(&graph, &text)?
        } else {
            resolve_decision_by_description(&graph, &text, None)?
        };
        match response.data {
            ResolveOutcome::Resolved { candidate } => {
                // The same premise named twice is one premise, not two identical edges.
                if !premise_decision_ids.contains(&candidate.decision_id) {
                    premise_decision_ids.push(candidate.decision_id);
                    premise_titles.push(candidate.title);
                }
            }
            data => {
                return Ok(GroundingResolution::Unresolved(UnresolvedPremise {
                    field,
                    text,
                    by_id,
                    response: QueryResponse { data, ..response },
                }))
            }
        }
    }

    Ok(GroundingResolution::Ready(ResolvedGrounding {
        plan: GroundingPlan {
            premise_decision_ids,
            evidence_ids: dedup_keeping_order(spec.evidence_ids),
            hypothesis_ids: dedup_keeping_order(spec.hypothesis_ids),
            new_evidence: spec.evidence,
            new_assumptions: spec.assumptions,
            bet: spec.bet,
        },
        premise_titles,
    }))
}

fn dedup_keeping_order(ids: Vec<String>) -> Vec<String> {
    let mut seen = Vec::with_capacity(ids.len());
    for id in ids {
        if !seen.contains(&id) {
            seen.push(id);
        }
    }
    seen
}

#[cfg(test)]
mod tests;
