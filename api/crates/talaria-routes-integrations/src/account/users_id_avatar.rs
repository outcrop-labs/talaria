// GET /api/users/{id}/avatar. A person's uploaded profile photo, for any
// signed-in member of the org and for agents (their tak_ key or the fleet
// key) — the one place an upload is readable outside the conversation-scoped
// `can_access_upload`. It is narrow on purpose: the upload id comes from the
// person's own `users.avatar_upload_id` column, never from the URL, so only an
// upload its owner claimed as their photo (and that passed the image-type and
// size check at claim time) is ever served here. 404 when the person has no
// photo or the bytes are gone.
//
// The bytes go through `serve_upload` like every other upload. The cache is
// immutable: the effective-picture URL carries `?v=<upload id prefix>`, so a
// new photo is a new URL.

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::Response;

use talaria_agent_auth::agent_caller;
use talaria_error::internal;
use talaria_session::require_user;
use talaria_state::AppState;
use talaria_uploads::{get_upload, serve_upload, upload_not_found};
use talaria_users::user_avatar_upload;

const AVATAR_CACHE: &str = "private, max-age=31536000, immutable";

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    if agent_caller(&state.pg, &headers).await?.is_none() {
        require_user(&state, &headers).await?;
    }
    if let Some(gate) = talaria_params::uuid_gate_404(&id) {
        return Ok(gate);
    }
    let upload_id = match user_avatar_upload(&state.pg, &id).await {
        Ok(Some(u)) => u,
        Ok(None) => return Ok(upload_not_found()),
        Err(e) => return Ok(internal("[users/avatar] avatar lookup failed", e)),
    };
    let sb = state.secretbox().await.unwrap_or_default();
    let found = match get_upload(&state.pg, &sb, &upload_id).await {
        Ok(f) => f,
        Err(e) => return Ok(internal("[users/avatar] blob read failed", e)),
    };
    let Some((bytes, mime, filename)) = found else {
        return Ok(upload_not_found());
    };
    Ok(serve_upload(bytes, &mime, &filename, AVATAR_CACHE))
}
