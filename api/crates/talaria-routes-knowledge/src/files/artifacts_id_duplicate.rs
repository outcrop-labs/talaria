// /api/artifacts/{id}/duplicate. Copy one artifact — copy/paste's engine on
// the file side. A READ-level act (anyone who can see it can copy it; the
// source is never touched): the copy is the caller's, private, beside its
// source, under a "Copy of" name. See artifacts::duplicate_artifact for the
// full copy semantics (shared storage ref, no public/KB/Google linkage).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use talaria_api_facades::kb::perms::{ITEM_ARTIFACT, can_read, list_editors};
use talaria_artifacts::{duplicate_artifact, get_artifact, guarded};
use talaria_error::{house_error, internal};
use talaria_session::{require_user, who_of};
use talaria_state::AppState;

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let src = match get_artifact(&state.pg, &id).await {
        Ok(a) => a,
        Err(e) => return internal("[artifacts] read failed", e),
    };
    let Some(src) = src else {
        return house_error(StatusCode::NOT_FOUND, "not found");
    };
    let editors = match list_editors(&state.pg, ITEM_ARTIFACT, &src.id).await {
        Ok(e) => e,
        Err(e) => return internal("[artifacts] grants read failed", e),
    };
    let team_ids = match talaria_teams::team_ids_for_user(&state.pg, &user.id).await {
        Ok(v) => v,
        Err(e) => return internal("[artifacts] team membership read failed", e),
    };
    if !can_read(
        &guarded(&src),
        Some(&user.id),
        who_of(&user).as_deref(),
        &editors,
        &team_ids,
    ) {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    match duplicate_artifact(&state.pg, &src.id, &user.id, Some(&user.id)).await {
        Ok(Some(copy)) => Json(json!({ "artifact": copy })).into_response(),
        Ok(None) => house_error(StatusCode::NOT_FOUND, "not found"),
        Err(e) => internal("[artifacts] duplicate failed", e),
    }
}
