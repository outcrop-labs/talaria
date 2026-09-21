// /api/kb/docs/{id}/backlinks. Docs that link to this one ("linked from").
// Editor links point at /knowledge/<id>, so backlinks fall out of a substring
// match. Gated by the SAME per-doc ACL as reading the doc — backlink titles
// leak content.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use talaria_api_facades::kb::perms::can_read;
use talaria_api_facades::kb::{effective_doc_perms, get_backlinks, get_doc};
use talaria_error::{house_error, internal};
use talaria_session::{require_user, who_of};
use talaria_state::AppState;

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let doc = match get_doc(&state.pg, &id).await {
        Ok(d) => d,
        Err(e) => return Ok(internal("[kb] doc read failed", e)),
    };
    let Some(doc) = doc else {
        return Ok(house_error(StatusCode::NOT_FOUND, "not found"));
    };
    let eff = match effective_doc_perms(&state.pg, &doc).await {
        Ok(e) => e,
        Err(e) => return Ok(internal("[kb] perms read failed", e)),
    };
    let who = who_of(&user);
    let team_ids = match talaria_teams::team_ids_for_user(&state.pg, &user.id).await {
        Ok(v) => v,
        Err(e) => return Ok(internal("[kb] team membership read failed", e)),
    };
    if !can_read(
        &eff.perms,
        Some(&user.id),
        who.as_deref(),
        &eff.grants,
        &team_ids,
    ) {
        return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    Ok(match get_backlinks(&state.pg, &id).await {
        Ok(backlinks) => Json(json!({ "backlinks": backlinks })).into_response(),
        Err(e) => internal("[kb] backlink scan failed", e),
    })
}
