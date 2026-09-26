//! The failure-mode analysis, with the seven quality dimensions as conditions.
//!
//! [`get_failure_attribution`](crate::queries::get_failure_attribution) says which conditions go
//! with decisions that did not hold up: authorship, review depth, source, delegation, context
//! richness. This adds the profile's dimensions to that list, so "do decisions that recorded no
//! alternatives fail more often?" is answered with the same rules `score_decision` reports.
//!
//! For each dimension a decision falls in one group: the level its profile gave it (`none`,
//! `partial` or `solid`) or `not_assessed`. `not_assessed` is its own group and is never folded
//! into `none`: a dimension nothing could be said about is not a dimension with nothing on record.
//!
//! A level sorts decisions into groups and is never itself a failure. Whether a decision held up
//! is the outcome view's call (superseded, a premise that no longer stands, contested) and nothing
//! here changes it; how quickly a decision was superseded weighs nothing either way.
//!
//! # Placement
//! Layer 3, with the rest of the profile. The query underneath takes conditions from its caller
//! and never names the profile, so it stays a pure read and works without this module.
//!
//! # Cost
//! What the analysis costs (one outcome and one context read per decision, at most
//! `MAX_QUERY_RESULTS` decisions) plus one profile read per analysed decision: a handful of
//! anchored lookups each, no scan, no model, no network.

use crate::projector::GraphView;
use crate::queries::{
    get_failure_attribution_with, DecisionCondition, FailureAttributionRequest, FailureModeReport,
    QueryResponse,
};
use crate::Result;

use super::{quality_profile_of, Level, QualityProfile};

/// The group label of a dimension the profile could not assess.
pub const NOT_ASSESSED: &str = "not_assessed";

/// [`get_failure_attribution`](crate::queries::get_failure_attribution) with the profile's seven
/// dimensions in `by_condition`: one group per (dimension, level or `not_assessed`).
pub fn analyze_failure_modes(
    graph: &impl GraphView,
    request: &FailureAttributionRequest,
) -> Result<QueryResponse<FailureModeReport>> {
    get_failure_attribution_with(graph, request, |decision_id| {
        Ok(quality_profile_of(graph, decision_id)?
            .map(|profile| profile_conditions(&profile))
            .unwrap_or_default())
    })
}

/// The seven conditions a profile states, in [`Dimension::ALL`](super::Dimension::ALL) order: each
/// dimension by its wire name and the wire name of its level, or [`NOT_ASSESSED`].
pub fn profile_conditions(profile: &QualityProfile) -> Vec<DecisionCondition> {
    profile
        .iter()
        .map(|(dimension, assessment)| DecisionCondition {
            dimension: dimension.as_str().to_owned(),
            label: assessment
                .level()
                .map_or(NOT_ASSESSED, Level::as_str)
                .to_owned(),
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod tests;
