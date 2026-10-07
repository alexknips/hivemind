//! Curating a cell's restatements over HTTP with the caller's token (hivemind-h4kr), so nobody
//! needs the database password to link a decision recorded twice.
//!
//! - `GET /v1/restatements/proposals` lists the links `hivemind restatements propose` lists
//!   (the pair, the overlap, the shared title words), a page at a time. It writes nothing.
//! - `POST /v1/decisions/{id}/restatements` records ONE `SAME_AS` link, `{id}` (recorded later)
//!   to the body's `restates_id` (recorded earlier), as the bearer token's actor, with the rules
//!   of `restatements apply --link LATER=EARLIER`: a pair already linked, either way round,
//!   writes nothing; nothing is deleted or rewritten; an id that is not a recorded decision is
//!   refused.
//!
//! There is deliberately no route that links every proposal: a proposal is title words, not a
//! finding, and a wrong `SAME_AS` cannot be undone for reads (`relation.removed` is not
//! projected, `queries::same_as`), so each link is a person's or an agent's own call.

use std::sync::Arc;
use std::time::Instant;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Json, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::commands::{CommandContext, Commands};
use crate::events::EventProvenance;
use crate::queries::{QueryResponse, MAX_QUERY_RESULTS};
use crate::restatement::{propose_same_as_links, ProposedLink};

use super::auth::extract_ctx;
use super::graph::open_graph_from_ledger;
use super::{respond, respond_envelope, to_api_error, ApiError, ApiResult, AppState};

const PROPOSALS_DEFAULT_LIMIT: usize = 25;

#[derive(Debug, Deserialize)]
pub(super) struct ProposalParams {
    /// Only decisions filed under this project.
    project: Option<String>,
    limit: Option<usize>,
    /// The `next_cursor` of the previous page.
    cursor: Option<String>,
}

/// One page of the proposed links, in the order the scan finds them (ledger order of the later
/// decision), so a cursor is an offset into a list that only grows at its end.
#[derive(Debug, Serialize)]
struct ProposalPage {
    limit: usize,
    cursor: Option<String>,
    next_cursor: Option<String>,
    total_matches: usize,
    /// Decisions compared, so an empty page reads as "checked this many", not "checked nothing".
    decisions_scanned: usize,
    items: Vec<ProposedLink>,
}

/// `GET /v1/restatements/proposals[?project=&limit=&cursor=]`.
pub(super) async fn proposals_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<ProposalParams>,
) -> Response {
    let ctx = match extract_ctx(&state, &headers).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };

    let backend = Arc::clone(&state.backend);
    let cache = Arc::clone(&state.graph_cache);
    let result = tokio::task::spawn_blocking(move || -> ApiResult<_> {
        let started = Instant::now();
        let offset = match params.cursor.as_deref() {
            None => 0,
            Some(cursor) => cursor.parse::<usize>().map_err(|error| {
                ApiError::validation(format!("cursor must be a non-negative offset: {error}"))
            })?,
        };
        let limit = match params.limit {
            None | Some(0) => PROPOSALS_DEFAULT_LIMIT,
            Some(limit) => limit.min(MAX_QUERY_RESULTS),
        };

        let ledger = backend.open_ledger_for_tenant(&ctx.tenant_id)?;
        let graph = open_graph_from_ledger(&ledger, &ctx.tenant_id, &cache)?;
        let proposals =
            propose_same_as_links(&*graph, params.project.as_deref()).map_err(to_api_error)?;

        let total_matches = proposals.proposed.len();
        let items: Vec<ProposedLink> = proposals
            .proposed
            .into_iter()
            .skip(offset)
            .take(limit)
            .collect();
        let next_offset = offset.saturating_add(items.len());
        let next_cursor = (next_offset < total_matches).then(|| next_offset.to_string());
        Ok(QueryResponse {
            result_count: items.len(),
            truncated: next_cursor.is_some(),
            latency_ms: started.elapsed().as_millis(),
            data: ProposalPage {
                limit,
                cursor: params.cursor,
                next_cursor,
                total_matches,
                decisions_scanned: proposals.decisions_scanned,
                items,
            },
        })
    })
    .await;

    respond_envelope(result)
}

/// The body of `POST /v1/decisions/{id}/restatements`: the one decision `{id}` restates. Any
/// other field is refused, so a client cannot mistake this for a bulk route.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LinkRestatementRequest {
    /// The decision recorded earlier that `{id}` states again.
    restates_id: String,
}

/// `POST /v1/decisions/{id}/restatements`: records that `{id}` is the same decision as
/// `restates_id`, as `relation.added SAME_AS`, attributed to the caller. Replies with the
/// `relation.added` event id, or `null` and `linked: false` when the two were already linked.
pub(super) async fn link_restatement_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(decision_id): Path<String>,
    payload: std::result::Result<Json<LinkRestatementRequest>, JsonRejection>,
) -> Response {
    let ctx = match extract_ctx(&state, &headers).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };
    let req = match payload {
        Ok(Json(r)) => r,
        Err(e) => return ApiError::validation(e.to_string()).into_response(),
    };

    let backend = Arc::clone(&state.backend);
    let result = tokio::task::spawn_blocking(move || -> ApiResult<_> {
        let ledger = backend.open_ledger_for_tenant(&ctx.tenant_id)?;
        let commands = Commands::new_with_context(
            &ledger,
            CommandContext::new(
                ctx.tenant_id.clone(),
                EventProvenance::api(Some(ctx.actor_id.clone())),
            ),
        );
        let event_id = commands
            .link_same_as(&ctx.actor_id, &decision_id, &req.restates_id)
            .map_err(to_api_error)?;
        Ok(serde_json::json!({
            "decision_id": decision_id,
            "restates_id": req.restates_id,
            "linked": event_id.is_some(),
            "event_id": event_id,
        }))
    })
    .await;

    respond(result, StatusCode::OK)
}
