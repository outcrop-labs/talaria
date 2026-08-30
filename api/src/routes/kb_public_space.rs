// /api/kb/public/space/{slug} — port of ui/src/routes/api/kb.public.space.$slug.ts.
// Public folder read — no auth. Only spaces with visibility 'public' resolve;
// returns the folder's name + overview (its body), like a public doc.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::error::house_error;
use crate::kb::get_public_space;
use crate::state::AppState;

pub async fn get(State(state): State<AppState>, Path(slug): Path<String>) -> Response {
    let space = match get_public_space(&state.pg, &slug).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("[kb] public space read failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    let Some(space) = space else {
        return house_error(StatusCode::NOT_FOUND, "not found");
    };
    Json(json!({
        "space": {
            "name": space.name,
            "icon": space.icon,
            "body": space.body,
        }
    }))
    .into_response()
}
