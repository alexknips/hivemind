mod active_blockers;
mod decision;
mod history;
mod neighborhood;
mod relevant;
mod search;
mod shared;
mod status;
mod supersession;

use serde::Serialize;

#[cfg(test)]
pub(crate) use shared::query_error;
pub(crate) use shared::MAX_QUERY_RESULTS;

pub use active_blockers::{
    get_active_decision_blockers, get_blocker_notification_candidates,
    ActiveDecisionBlockersRequest, BlockerNotificationCandidate, BlockerNotificationCandidates,
    BlockerNotificationCandidatesRequest, BlockerNotificationState, BlockerNotificationStateKind,
    DecisionBlockerFilters, DecisionBlockerResults, DecisionBlockerView,
};
pub use decision::{get_decision, DecisionView, HypothesisContext};
pub use history::*;
pub use neighborhood::{
    get_decision_neighborhood, NeighborEdge, NeighborNode, NeighborhoodRequest, NeighborhoodRoot,
    NeighborhoodView,
};
pub use relevant::get_relevant_decisions;
pub use search::{
    search_decisions, DecisionSearchResult, DecisionSearchResults, SearchDecisionFilters,
    SearchDecisionRequest, SearchGraphContext, SearchMatchedNode, SearchSnippet,
};
pub use status::{
    derive_decision_status, derive_hypothesis_status, DecisionStatus, HypothesisStatus,
};
pub use supersession::{get_supersession_chain, SupersessionChain};

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct QueryResponse<T> {
    pub result_count: usize,
    pub truncated: bool,
    pub latency_ms: u128,
    pub data: T,
}

#[cfg(test)]
mod tests;
