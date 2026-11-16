// /api/workbench/git-hosts/{id}/refresh. The host card's "Refresh": POST
// reads the host's GitHub App (what it asks for) and each installation its
// connection uses (what the installation accepted) again, now, instead of at
// the next ten-minute pass, and answers the host as GET does. A permission an
// installation no longer holds revokes the tokens that carry it, as on every
// refresh (talaria-git-hosts' holdings). A host with no App to read answers
// unchanged. Every call to GitHub underneath is a read. requireAdmin, like
// the rest of the host plane.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use talaria_error::{house_error, internal};
use talaria_git_hosts::registry;
use talaria_session::require_admin;
use talaria_state::AppState;

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    let row = match registry::get_host(&state.pg, &id).await {
        Ok(Some(row)) => row,
        Ok(None) => return Ok(house_error(StatusCode::NOT_FOUND, "no such git host")),
        Err(e) => return Ok(internal("[workbench/git-hosts] host read failed", e)),
    };
    let sb = state.secretbox().await.unwrap_or_default();
    // GitHub failing is recorded on the rows and shown on the card; only
    // the record itself failing to write is an error here.
    if let Err(e) = talaria_git_hosts::refresh_host(&state.pg, &sb, &row.id).await {
        return Ok(internal("[workbench/git-hosts] holdings refresh failed", e));
    }
    let host = talaria_git_hosts::connect(&state.pg, &sb, &row).await;
    let st = talaria_git_hosts::host_status(&state.pg, &host, &row).await;
    Ok(Json(json!({ "host": row.wire_with(&st) })).into_response())
}
