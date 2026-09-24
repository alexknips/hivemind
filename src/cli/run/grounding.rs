//! CLI side of grounded capture (hivemind-gwhr.2): the `--rests-on-*` / `--bet` flags of
//! `emit decision.capture` and `supersede`, turned into a `GroundingSpec`, resolved, and
//! refused — with nothing written — when they name nothing or name a premise the resolver cannot
//! pin to one decision.

use std::fmt::Write as _;
use std::path::Path;

use crate::cli::args::GroundingArgs;
use crate::commands::{NewBet, NewEvidence};
use crate::error::CliError;
use crate::events::TenantId;
use crate::grounding::{
    resolve_grounding, GroundingResolution, GroundingSpec, PremiseSpec, PremiseTarget,
    ResolvedGrounding, UnresolvedPremise,
};
use crate::ledger::EventLedger;
use crate::Result;

use super::{
    candidate_handle_index, parse_check_by_date, read_continuation_candidate,
    write_continuation_candidates,
};

/// The refusal for a capture that names nothing it rests on: the four ways to answer, the
/// existing-id escape hatches, and where the decider's own words go instead.
pub(super) const GROUNDING_REFUSAL: &str = "a captured decision must say what it rests on; nothing was written. Answer with at least one of: \
--rests-on-decision <description|#N|decision-id> (a decision we already made), \
--rests-on-evidence <what was observed> with --evidence-source <where> (something observed), \
--rests-on-assumption <statement> (something we assume), or \
--bet [statement] with optional --would-change-if / --check-by (nothing yet: a declared bet). \
Existing nodes also count: --evidence <id>, --hypotheses <id>. \
The decider's own words are not a grounding; they go in --quote (with --question)";

/// A literal decision id: `decision-` followed by no whitespace. Anything else is a description.
fn is_decision_id(text: &str) -> bool {
    text.starts_with("decision-") && !text.contains(char::is_whitespace)
}

fn trimmed_flag_value<'a>(flag: &'static str, value: &'a str) -> Result<&'a str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Err(CliError::InvalidInput(format!("{flag} must not be empty")).into())
    } else {
        Ok(trimmed)
    }
}

/// One `--rests-on-decision` value: a `#N` handle, a literal decision id, or a description.
fn premise_spec(hivemind_dir: &Path, raw: &str) -> Result<PremiseSpec> {
    let text = trimmed_flag_value("--rests-on-decision", raw)?;
    let target = if let Some(index) = candidate_handle_index(text) {
        PremiseTarget::Id(read_continuation_candidate(hivemind_dir, index)?.decision_id)
    } else if is_decision_id(text) {
        PremiseTarget::Id(text.to_owned())
    } else {
        PremiseTarget::Description(text.to_owned())
    };
    Ok(PremiseSpec {
        field: format!("--rests-on-decision '{text}'"),
        target,
    })
}

/// One `--rests-on-evidence` value with the `--evidence-source` at the same index, if any.
fn new_evidence(content: &str, source: Option<&String>) -> Result<NewEvidence> {
    Ok(NewEvidence {
        content: trimmed_flag_value("--rests-on-evidence", content)?.to_owned(),
        source: source
            .map(|source| trimmed_flag_value("--evidence-source", source).map(str::to_owned))
            .transpose()?,
    })
}

/// Collect the grounding flags into a spec. `#N` handles are read from the continuation file
/// here — before anything resolves a description and rewrites that file — so a re-run with
/// `--rests-on-decision '#2'` always means candidate 2 of the list the previous run printed.
///
/// `evidence_ids` / `hypothesis_ids` are the pre-existing `--evidence` / `--hypotheses` ids.
pub(super) fn grounding_spec_from_args(
    hivemind_dir: &Path,
    args: &GroundingArgs,
    evidence_ids: &[String],
    hypothesis_ids: &[String],
) -> Result<GroundingSpec> {
    if !args.evidence_sources.is_empty() {
        if args.rests_on_evidence.is_empty() {
            return Err(CliError::InvalidInput(
                "--evidence-source needs a matching --rests-on-evidence".to_owned(),
            )
            .into());
        }
        if args.evidence_sources.len() != args.rests_on_evidence.len() {
            return Err(CliError::InvalidInput(format!(
                "--evidence-source is index-aligned with --rests-on-evidence: got {} source(s) for {} evidence item(s); give one source per item, or none",
                args.evidence_sources.len(),
                args.rests_on_evidence.len()
            ))
            .into());
        }
    }

    let bet = match &args.bet {
        Some(statement) => Some(NewBet {
            statement: statement.as_deref().map(|text| text.trim().to_owned()),
            would_change_if: args
                .would_change_if
                .as_deref()
                .map(|text| trimmed_flag_value("--would-change-if", text).map(str::to_owned))
                .transpose()?,
            check_by: args
                .check_by
                .as_deref()
                .map(|value| parse_check_by_date("--check-by", value))
                .transpose()?,
        }),
        None => None,
    };

    Ok(GroundingSpec {
        decisions: args
            .rests_on_decisions
            .iter()
            .map(|raw| premise_spec(hivemind_dir, raw))
            .collect::<Result<_>>()?,
        evidence: args
            .rests_on_evidence
            .iter()
            .enumerate()
            .map(|(index, content)| new_evidence(content, args.evidence_sources.get(index)))
            .collect::<Result<_>>()?,
        evidence_ids: evidence_ids.to_vec(),
        assumptions: args
            .rests_on_assumptions
            .iter()
            .map(|statement| {
                trimmed_flag_value("--rests-on-assumption", statement).map(str::to_owned)
            })
            .collect::<Result<_>>()?,
        hypothesis_ids: hypothesis_ids.to_vec(),
        bet,
    })
}

/// Refuse a premise that is ambiguous or matches nothing. An ambiguous description leaves its
/// numbered candidates in the continuation file so the re-run can say `'#N'`.
fn refuse_unresolved(hivemind_dir: &Path, unresolved: &UnresolvedPremise) -> CliError {
    let text = &unresolved.text;
    let candidates = unresolved.candidates();
    if candidates.is_empty() {
        return CliError::InvalidInput(format!(
            "no decision matches '{text}'; nothing was written"
        ));
    }

    write_continuation_candidates(hivemind_dir, candidates);
    // No decision containing every word means every candidate is a close one that lacks some.
    let close = candidates
        .iter()
        .all(|candidate| !candidate.missing_terms.is_empty());
    let mut message = if close {
        format!(
            "no decision matches every word of '{text}'; {} are close and none is picked for you; nothing was written.",
            candidates.len()
        )
    } else {
        format!(
            "'{text}' matches {} decisions and none is clearly the one; nothing was written.",
            candidates.len()
        )
    };
    for (index, candidate) in candidates.iter().enumerate() {
        let _ = write!(
            message,
            "\n  #{} {} ({})",
            index + 1,
            candidate.title,
            candidate.decision_id
        );
        if !candidate.missing_terms.is_empty() {
            message.push_str(" missing:");
            for term in &candidate.missing_terms {
                let _ = write!(message, " {term}");
            }
        }
    }
    message.push_str(
        "\nre-run with --rests-on-decision '#N' (or the decision id) in place of the description",
    );
    CliError::InvalidInput(message)
}

/// Refuse a spec that names nothing the decision rests on. Checked before anything is resolved
/// or written.
pub(super) fn require_grounding(spec: GroundingSpec) -> Result<GroundingSpec> {
    if spec.is_empty() {
        return Err(CliError::InvalidInput(GROUNDING_REFUSAL.to_owned()).into());
    }
    Ok(spec)
}

/// Resolve the spec's premise decisions. The refusal for an ambiguous or unmatched premise is
/// returned as the error; nothing has been written by then.
pub(super) fn resolve_declared_grounding<L: EventLedger>(
    hivemind_dir: &Path,
    ledger: &L,
    tenant_id: &TenantId,
    spec: GroundingSpec,
) -> Result<ResolvedGrounding> {
    match resolve_grounding(ledger, tenant_id, spec)? {
        GroundingResolution::Ready(resolved) => Ok(resolved),
        GroundingResolution::Unresolved(unresolved) => {
            Err(refuse_unresolved(hivemind_dir, &unresolved).into())
        }
    }
}
