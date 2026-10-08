//! `POST /v1/ledger/replay` — append events from another ledger to a tenant, keeping each event's
//! uuid, actor, source, source ref, correlation id, payload and time (hivemind-jawy).
//!
//! This is how a ledger moves into a cell without anyone holding the cell's database password:
//! `hivemind migrate --to-url` reads the source and sends it here in batches. Auth is the admin
//! key (`HIVEMIND_ADMIN_KEY`), not a tenant token, because the events carry their own actors:
//! the route records who each event was by, not who sent it. The rules (dedup by event uuid, a
//! causation link numbered by its cause's uuid, a batch with an unresolvable link writes
//! nothing) are those of [`crate::replay::replay_events`].

use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Json, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::events::TenantId;
use crate::replay::{replay_events, ReplayEvent};

use super::auth::check_admin_key;
use super::{respond, to_api_error, ApiError, AppState};

/// Largest request body the replay route reads. The default 2 MiB limit of the other routes would
/// refuse a batch carrying a long ingested transcript.
pub(super) const MAX_REPLAY_BODY_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReplayRequest {
    /// The tenant the events land in. Named on every request: a replay into the wrong tenant
    /// cannot be undone.
    tenant_id: String,
    /// Count which events are new and write nothing.
    #[serde(default)]
    dry_run: bool,
    events: Vec<ReplayEvent>,
}

pub(super) async fn replay_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    payload: std::result::Result<Json<ReplayRequest>, JsonRejection>,
) -> Response {
    if let Err(response) = check_admin_key(&state, &headers) {
        return response;
    }
    let req = match payload {
        Ok(Json(req)) => req,
        Err(e) => return ApiError::validation(e.to_string()).into_response(),
    };
    let tenant_id = match TenantId::new(req.tenant_id.trim().to_owned()) {
        Ok(tenant_id) => tenant_id,
        Err(e) => {
            return ApiError::validation(format!("tenant_id is invalid: {e}")).into_response()
        }
    };

    let backend = Arc::clone(&state.backend);
    let result = tokio::task::spawn_blocking(move || {
        let ledger = backend.open_ledger_for_tenant(&tenant_id)?;
        let counts =
            replay_events(&ledger, &tenant_id, req.events, req.dry_run).map_err(to_api_error)?;
        Ok::<_, ApiError>(serde_json::json!({
            "tenant_id": tenant_id.as_str(),
            "dry_run": req.dry_run,
            "received": counts.received,
            "new_events": counts.new_events,
            "already_present": counts.already_present,
        }))
    })
    .await;

    respond(result, StatusCode::OK)
}
